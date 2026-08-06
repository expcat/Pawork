//! 结构化 Diff 数据模型（与 `docs/features/git-diff.md` 对齐）。

/// 文件级变更状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    #[default]
    Unmodified,
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
    Untracked,
}

/// 单行 diff 的类型。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    Context,
    #[default]
    Addition,
    Deletion,
}

/// Hunk 标识（crate 内自增分配）。
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct HunkId(pub u64);

/// diff 中的一行。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DiffLine {
    pub kind: LineKind,
    /// 行文本（不含行首的 `+`/`-`/` ` 前缀）。
    pub text: String,
    /// 该行对应的新文件侧无末尾换行（git 的 `\ No newline at end of file`）。
    pub new_no_newline: bool,
}

/// 一个 diff hunk。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DiffHunk {
    pub id: HunkId,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// hunk 头原文，如 `@@ -1,3 +1,4 @@`。
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// 一个文件的 diff。
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DiffFile {
    /// 当前路径。
    pub path: String,
    /// rename / copy 时的原始路径。
    pub previous_path: Option<String>,
    pub status: FileStatus,
    /// `true` 表示对比 index（`--cached`）；`false` 表示对比 worktree。
    pub staged: bool,
    pub binary: bool,
    pub additions: u32,
    pub deletions: u32,
    pub hunks: Vec<DiffHunk>,
}

impl DiffFile {
    /// 该文件的总变更行数。
    pub fn changed_lines(&self) -> u32 {
        self.additions + self.deletions
    }
}
