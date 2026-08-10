//! Worker 结果聚合 / patch merge / 冲突检测（P12-5）。
//!
//! **Parent 审批门**：是否合并由 [`MergeDecision`] 表达，该决策由调用方
//! （parent agent / 人）提供。[`PatchMerger`] 绝不自动合并冲突：遇到冲突
//! 一律返回 [`MergeError::ConflictUnresolved`]，等待 Parent 决定。
//!
//! `OrchestrationEvent::PatchProposed / PatchMerged / PatchConflict` 由
//! 调用方（编排宿主）依据本模块结果发出；本模块本身无事件日志。

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use agent_domain::{AgentId, CancellationToken, SessionId};
use async_trait::async_trait;
use diff_service::{DiffOptions, DiffService};
use git_service::GitRunner;

/// 一次 worker 结果聚合的输入。
#[derive(Clone, Debug)]
pub struct WorkerPatch {
    /// 提出 patch 的 agent。
    pub agent_id: AgentId,
    /// 会话。
    pub session_id: SessionId,
    /// worker worktree 路径。
    pub worktree_path: PathBuf,
    /// 变更文件列表。
    pub changed_files: Vec<String>,
}

/// Parent 审批结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeDecision {
    /// 批准合并；冲突未解决时 [`PatchMerger`] 拒绝而非自动合。
    Merge,
    /// 拒绝并附原因（不写任何文件）。
    Reject {
        /// 拒绝原因。
        reason: String,
    },
    /// 需要人工 / parent 解决冲突后再来。
    NeedsConflictResolution {
        /// 待解决冲突文件。
        files: Vec<String>,
    },
}

/// 合并错误。
#[derive(Debug, thiserror::Error)]
pub enum MergeError {
    /// I/O 错误（读 / 写 / 原子重命名）。
    #[error("I/O error{context}: {source}")]
    Io {
        /// 上下文描述。
        context: String,
        /// 底层 I/O 错误。
        #[source]
        source: std::io::Error,
    },
    /// diff / 内容获取失败。
    #[error("diff error: {0}")]
    Diff(String),
    /// 发现冲突但审批未解决：不允许自动合并。
    #[error("conflict unresolved for files: {files:?}")]
    ConflictUnresolved {
        /// 冲突文件。
        files: Vec<String>,
    },
}

/// 变更来源抽象：真实实现包装 `diff-service` + `std::fs`，测试注入 fake。
#[async_trait]
pub trait DiffProvider: Send + Sync {
    /// 返回 worktree 相对 `HEAD` 变更的文件相对路径列表。
    async fn changed_files(&self, worktree_path: &Path) -> Result<Vec<String>, MergeError>;

    /// 读取 `rel` 相对 worktree 根的文件内容。
    async fn file_content(&self, worktree_path: &Path, rel: &str) -> Result<Vec<u8>, MergeError>;

    /// 补丁基准内容：worker fork 时父侧该文件的原始内容。
    ///
    /// 冲突检测用父侧当前内容与基准比较。默认实现退化为父侧当前内容
    /// （适用于父侧不可能变动的场景）；真实实现应提供 fork 点内容
    /// （如 `git show HEAD:<rel>`），以保证并发合并时冲突检测正确。
    async fn base_content(&self, parent_path: &Path, rel: &str) -> Result<Vec<u8>, MergeError> {
        self.file_content(parent_path, rel).await
    }
}

/// 真实 DiffProvider：变更清单走 `diff-service`，文件内容走 `std::fs`，
/// 基准内容走 `git show HEAD:<rel>`。
pub struct GitDiffProvider {
    cancel: CancellationToken,
}

impl GitDiffProvider {
    /// 新建提供者（默认取消令牌）。
    pub fn new() -> Self {
        Self {
            cancel: CancellationToken::new(),
        }
    }

    /// 覆盖取消令牌。
    pub fn with_cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }
}

impl Default for GitDiffProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DiffProvider for GitDiffProvider {
    async fn changed_files(&self, worktree_path: &Path) -> Result<Vec<String>, MergeError> {
        let service = DiffService::new(GitRunner::new(), worktree_path);
        let options = DiffOptions {
            commit_range: Some("HEAD".to_string()),
            ..DiffOptions::default()
        };
        let files = service
            .diff_summary(&options, self.cancel.clone())
            .await
            .map_err(|error| MergeError::Diff(error.to_string()))?;
        Ok(files.into_iter().map(|file| file.path).collect())
    }

    async fn file_content(&self, worktree_path: &Path, rel: &str) -> Result<Vec<u8>, MergeError> {
        let path = resolve_relative(worktree_path, rel)?;
        std::fs::read(&path).map_err(|source| MergeError::Io {
            context: format!(" while reading {}", path.display()),
            source,
        })
    }

    async fn base_content(&self, parent_path: &Path, rel: &str) -> Result<Vec<u8>, MergeError> {
        let runner = GitRunner::new();
        let arg = format!("HEAD:{rel}");
        match runner
            .run(parent_path, &["show", &arg], self.cancel.clone())
            .await
        {
            Ok(content) => Ok(content.into_bytes()),
            Err(_) => self.file_content(parent_path, rel).await,
        }
    }
}

/// 一次已收集的 patch 提案：「worker 最终内容」用于合并写出。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchProposal {
    /// 提案 agent。
    pub agent_id: AgentId,
    /// 变更文件相对路径。
    pub files: Vec<String>,
    /// 每个文件的最终内容。
    pub contents: BTreeMap<String, Vec<u8>>,
}

/// 冲突检测报告。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConflictReport {
    /// 父侧已变更 / 与基准不一致的文件（需要 Parent 决定）。
    pub conflicting_files: Vec<String>,
    /// 可安全合并的文件。
    pub clean_files: Vec<String>,
}

impl ConflictReport {
    /// 是否存在冲突。
    pub fn has_conflicts(&self) -> bool {
        !self.conflicting_files.is_empty()
    }
}

/// 合并结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeOutcome {
    /// 已合入 parent 的文件。
    pub merged_files: Vec<String>,
    /// 被跳过（Reject / 待解决）的文件。
    pub skipped_files: Vec<String>,
    /// 冲突文件。
    pub conflicts: Vec<String>,
}

/// Patch 合并器。
pub struct PatchMerger {
    diff: Arc<dyn DiffProvider>,
}

impl PatchMerger {
    /// 以注入的变更来源构造。
    pub fn new(diff: Arc<dyn DiffProvider>) -> Self {
        Self { diff }
    }

    /// 收集 worker 变更：变更文件清单 + 每个文件的最终内容。
    pub async fn collect(&self, patch: &WorkerPatch) -> Result<PatchProposal, MergeError> {
        let files = self.diff.changed_files(&patch.worktree_path).await?;
        let mut contents = BTreeMap::new();
        for file in &files {
            let content = self.diff.file_content(&patch.worktree_path, file).await?;
            contents.insert(file.clone(), content);
        }
        Ok(PatchProposal {
            agent_id: patch.agent_id.clone(),
            files,
            contents,
        })
    }

    /// 冲突检测：对每个 worker 改动的文件，比较父侧当前内容与补丁基准。
    ///
    /// 规则：文件在父侧不存在 → 干净（无父侧版本可被覆盖）；
    /// 父侧当前哈希与基准一致 → 干净；不一致（父侧已变动）→ 冲突。
    pub async fn detect_conflicts(
        &self,
        proposal: &PatchProposal,
        parent_path: &Path,
    ) -> Result<ConflictReport, MergeError> {
        let mut conflicting = Vec::new();
        let mut clean = Vec::new();
        for file in &proposal.files {
            let parent_file = parent_path.join(file);
            let parent_content = match std::fs::read(&parent_file) {
                Ok(bytes) => bytes,
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                    clean.push(file.clone());
                    continue;
                }
                Err(source) => {
                    return Err(MergeError::Io {
                        context: format!(" while reading {}", parent_file.display()),
                        source,
                    });
                }
            };
            let base = self.diff.base_content(parent_path, file).await?;
            if blake3::hash(&parent_content) != blake3::hash(&base) {
                conflicting.push(file.clone());
            } else {
                clean.push(file.clone());
            }
        }
        Ok(ConflictReport {
            conflicting_files: conflicting,
            clean_files: clean,
        })
    }

    /// 依据 Parent 决策执行合并。
    ///
    /// - [`MergeDecision::Merge`]：冲突未解决则拒绝（绝不自动合并）；
    ///   否则把干净文件以原子写（同目录 tmp + rename）合入 parent。
    /// - [`MergeDecision::Reject`]：不写任何文件。
    /// - [`MergeDecision::NeedsConflictResolution`]：不写任何文件，返回冲突。
    pub async fn merge(
        &self,
        proposal: &PatchProposal,
        parent_path: &Path,
        decision: &MergeDecision,
    ) -> Result<MergeOutcome, MergeError> {
        match decision {
            MergeDecision::Reject { .. } => Ok(MergeOutcome {
                merged_files: Vec::new(),
                skipped_files: proposal.files.clone(),
                conflicts: Vec::new(),
            }),
            MergeDecision::NeedsConflictResolution { files } => Ok(MergeOutcome {
                merged_files: Vec::new(),
                skipped_files: proposal.files.clone(),
                conflicts: files.clone(),
            }),
            MergeDecision::Merge => {
                let report = self.detect_conflicts(proposal, parent_path).await?;
                if report.has_conflicts() {
                    return Err(MergeError::ConflictUnresolved {
                        files: report.conflicting_files,
                    });
                }
                let mut merged = Vec::new();
                for file in &proposal.files {
                    let content = proposal.contents.get(file).ok_or_else(|| {
                        MergeError::Diff(format!("missing proposed content for {file}"))
                    })?;
                    atomic_write(&parent_path.join(file), content)?;
                    merged.push(file.clone());
                }
                Ok(MergeOutcome {
                    merged_files: merged,
                    skipped_files: Vec::new(),
                    conflicts: Vec::new(),
                })
            }
        }
    }
}

/// 在 `root` 内解析相对路径：拒绝绝对路径与 `..` 穿越。
fn resolve_relative(root: &Path, rel: &str) -> Result<PathBuf, MergeError> {
    if rel.is_empty() {
        return Err(MergeError::Diff("empty relative path".to_string()));
    }
    for component in Path::new(rel).components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                return Err(MergeError::Diff(format!("absolute component in {rel:?}")));
            }
            Component::ParentDir => {
                return Err(MergeError::Diff(format!("parent traversal in {rel:?}")));
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    Ok(root.join(rel))
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 原子写：同目录临时文件写入 + `rename`（失败时清理临时文件）。
fn atomic_write(path: &Path, content: &[u8]) -> Result<(), MergeError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| MergeError::Io {
        context: format!(" while creating {}", parent.display()),
        source,
    })?;
    let temp = path.with_file_name(format!(
        ".merge-tmp-{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = std::fs::write(&temp, content).and_then(|_| std::fs::rename(&temp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map_err(|source| MergeError::Io {
        context: format!(" while writing {}", path.display()),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 测试用 fake：worktree 最终内容 + 基准内容的脚本表。
    #[derive(Clone)]
    pub struct FakeDiffProvider {
        files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
        base: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    }

    impl FakeDiffProvider {
        pub fn new(files: BTreeMap<String, Vec<u8>>) -> Self {
            Self {
                files: Arc::new(Mutex::new(files)),
                base: Arc::new(Mutex::new(BTreeMap::new())),
            }
        }

        pub fn with_base(self, base: BTreeMap<String, Vec<u8>>) -> Self {
            *self
                .base
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = base;
            self
        }
    }

    #[async_trait]
    impl DiffProvider for FakeDiffProvider {
        async fn changed_files(&self, _worktree_path: &Path) -> Result<Vec<String>, MergeError> {
            Ok(self
                .files
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .keys()
                .cloned()
                .collect())
        }

        async fn file_content(
            &self,
            _worktree_path: &Path,
            rel: &str,
        ) -> Result<Vec<u8>, MergeError> {
            self.files
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .get(rel)
                .cloned()
                .ok_or_else(|| MergeError::Diff(format!("no such file {rel}")))
        }

        async fn base_content(
            &self,
            _parent_path: &Path,
            rel: &str,
        ) -> Result<Vec<u8>, MergeError> {
            self.base
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .get(rel)
                .cloned()
                .ok_or_else(|| MergeError::Diff(format!("no base for {rel}")))
        }
    }

    fn patch(agent: &str) -> WorkerPatch {
        WorkerPatch {
            agent_id: AgentId::new(agent),
            session_id: SessionId::new("session-1"),
            worktree_path: PathBuf::from("/wt"),
            changed_files: Vec::new(),
        }
    }

    #[tokio::test]
    async fn collect_gathers_changed_files_and_content() {
        let provider = FakeDiffProvider::new(BTreeMap::from([
            ("a.txt".to_string(), b"worker-v1".to_vec()),
            ("b.txt".to_string(), b"created".to_vec()),
        ]));
        let merger = PatchMerger::new(Arc::new(provider));
        let proposal = merger.collect(&patch("agent-1")).await.unwrap();
        assert_eq!(proposal.agent_id, AgentId::new("agent-1"));
        assert_eq!(
            proposal.files,
            vec!["a.txt".to_string(), "b.txt".to_string()]
        );
        assert_eq!(proposal.contents["a.txt"], b"worker-v1");
        assert_eq!(proposal.contents["b.txt"], b"created");
    }

    #[tokio::test]
    async fn detect_conflicts_flags_diverged_parent_only() {
        let parent = tempfile::tempdir().unwrap();
        // worker 的基准即 base 内容；父侧初始与基准一致。
        std::fs::write(parent.path().join("a.txt"), b"base").unwrap();
        let provider = FakeDiffProvider::new(BTreeMap::from([
            ("a.txt".to_string(), b"worker-v1".to_vec()),
            ("b.txt".to_string(), b"new-file".to_vec()),
        ]))
        .with_base(BTreeMap::from([
            ("a.txt".to_string(), b"base".to_vec()),
            ("b.txt".to_string(), Vec::new()),
        ]));
        let merger = PatchMerger::new(Arc::new(provider));
        let proposal = merger.collect(&patch("agent-1")).await.unwrap();

        // 父侧未变：a 干净；b 在父侧不存在：干净。
        let report = merger
            .detect_conflicts(&proposal, parent.path())
            .await
            .unwrap();
        assert!(!report.has_conflicts());
        assert_eq!(
            report.clean_files,
            vec!["a.txt".to_string(), "b.txt".to_string()]
        );

        // 父侧 a.txt 被并发修改 → 冲突。
        std::fs::write(parent.path().join("a.txt"), b"parent-edit").unwrap();
        let report = merger
            .detect_conflicts(&proposal, parent.path())
            .await
            .unwrap();
        assert!(report.has_conflicts());
        assert_eq!(report.conflicting_files, vec!["a.txt".to_string()]);
        assert_eq!(report.clean_files, vec!["b.txt".to_string()]);
    }

    #[tokio::test]
    async fn merge_copies_clean_files_atomically() {
        let parent = tempfile::tempdir().unwrap();
        std::fs::write(parent.path().join("a.txt"), b"base").unwrap();
        let provider = FakeDiffProvider::new(BTreeMap::from([
            ("a.txt".to_string(), b"worker-v1".to_vec()),
            ("new.txt".to_string(), b"hey".to_vec()),
        ]))
        .with_base(BTreeMap::from([
            ("a.txt".to_string(), b"base".to_vec()),
            ("new.txt".to_string(), Vec::new()),
        ]));
        let merger = PatchMerger::new(Arc::new(provider));
        let proposal = merger.collect(&patch("agent-1")).await.unwrap();

        let outcome = merger
            .merge(&proposal, parent.path(), &MergeDecision::Merge)
            .await
            .unwrap();
        assert_eq!(outcome.merged_files.len(), 2);
        assert!(outcome.conflicts.is_empty());
        assert_eq!(
            std::fs::read(parent.path().join("a.txt")).unwrap(),
            b"worker-v1"
        );
        assert_eq!(
            std::fs::read(parent.path().join("new.txt")).unwrap(),
            b"hey"
        );
    }

    #[tokio::test]
    async fn merge_conflict_never_auto_merges() {
        let parent = tempfile::tempdir().unwrap();
        std::fs::write(parent.path().join("a.txt"), b"base").unwrap();
        let provider = FakeDiffProvider::new(BTreeMap::from([(
            "a.txt".to_string(),
            b"worker-v1".to_vec(),
        )]))
        .with_base(BTreeMap::from([("a.txt".to_string(), b"base".to_vec())]));
        let merger = PatchMerger::new(Arc::new(provider));
        let proposal = merger.collect(&patch("agent-1")).await.unwrap();

        // 父侧被并发修改。
        std::fs::write(parent.path().join("a.txt"), b"parent-edit").unwrap();
        let err = merger
            .merge(&proposal, parent.path(), &MergeDecision::Merge)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            MergeError::ConflictUnresolved { ref files } if files == &vec!["a.txt".to_string()]
        ));
        assert_eq!(
            std::fs::read_to_string(parent.path().join("a.txt")).unwrap(),
            "parent-edit",
            "冲突时绝不允许自动合并覆盖父侧内容"
        );
    }

    #[tokio::test]
    async fn reject_is_noop_with_reason() {
        let parent = tempfile::tempdir().unwrap();
        std::fs::write(parent.path().join("a.txt"), b"base").unwrap();
        let provider = FakeDiffProvider::new(BTreeMap::from([(
            "a.txt".to_string(),
            b"worker-v1".to_vec(),
        )]))
        .with_base(BTreeMap::from([("a.txt".to_string(), b"base".to_vec())]));
        let merger = PatchMerger::new(Arc::new(provider));
        let proposal = merger.collect(&patch("agent-1")).await.unwrap();

        let outcome = merger
            .merge(
                &proposal,
                parent.path(),
                &MergeDecision::Reject {
                    reason: "not wanted".into(),
                },
            )
            .await
            .unwrap();
        assert!(outcome.merged_files.is_empty());
        assert_eq!(outcome.skipped_files, vec!["a.txt".to_string()]);
        assert_eq!(
            std::fs::read_to_string(parent.path().join("a.txt")).unwrap(),
            "base",
            "Reject 不得写入任何文件"
        );
    }

    #[tokio::test]
    async fn needs_conflict_resolution_writes_nothing() {
        let parent = tempfile::tempdir().unwrap();
        std::fs::write(parent.path().join("a.txt"), b"base").unwrap();
        let provider = FakeDiffProvider::new(BTreeMap::from([(
            "a.txt".to_string(),
            b"worker-v1".to_vec(),
        )]))
        .with_base(BTreeMap::from([("a.txt".to_string(), b"base".to_vec())]));
        let merger = PatchMerger::new(Arc::new(provider));
        let proposal = merger.collect(&patch("agent-1")).await.unwrap();

        let outcome = merger
            .merge(
                &proposal,
                parent.path(),
                &MergeDecision::NeedsConflictResolution {
                    files: vec!["a.txt".to_string()],
                },
            )
            .await
            .unwrap();
        assert!(outcome.merged_files.is_empty());
        assert_eq!(outcome.conflicts, vec!["a.txt".to_string()]);
        assert_eq!(
            std::fs::read_to_string(parent.path().join("a.txt")).unwrap(),
            "base"
        );
    }

    #[test]
    fn relative_path_with_parent_traversal_rejected() {
        let err = resolve_relative(Path::new("/repo"), "../escape.txt").unwrap_err();
        assert!(matches!(err, MergeError::Diff(_)));
        let err = resolve_relative(Path::new("/repo"), "/abs.txt").unwrap_err();
        assert!(matches!(err, MergeError::Diff(_)));
        let ok = resolve_relative(Path::new("/repo"), "sub/dir/file.txt").unwrap();
        assert_eq!(ok, Path::new("/repo/sub/dir/file.txt"));
    }
}
