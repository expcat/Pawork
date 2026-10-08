//! Pawork 系统 Git 封装与结构化 Diff。
//!
//! 本 crate 以系统 `git` 为唯一后端，统一经
//! [`pawork_exec::ProcessRuntime`] 执行命令，并把 `ProcessError`、非零退出、
//! 超时与取消归一为 [`GitError`]。
//!
//! 模块：
//! - [`error`]：错误归一。
//! - [`process`]：[`GitRunner`] 系统 git 统一调用入口。
//! - [`repo`]：[`GitService`] 仓库检测与 HEAD/branch 元信息。
//! - [`status`]：[`status::StatusService`] 工作区状态解析。
//! - [`worktree`]：[`worktree::WorktreeService`] Worktree 创建/删除（清理不删用户数据）。
//! - [`diff`]：结构化 Diff（`DiffFile`/`DiffHunk`/`DiffLine`）。
//!
//! R0/ADR-038 D16（2026-08-18 波 C）：branch/stash/conflict/history/cache/commit
//! 六个零消费服务已归档删除（git tag `v2-final` 可找回），复活条件见 docs/spec/backlog.md。
//! B12（2026-10-08）：StageService / HunkStageService 无生产消费者，已归档删除。
//! 当前布局的完整源码在 `44a8848ed21caeb02fa6318332f4f882df951e3e` 的
//! `crates/git/src/stage.rs` 与 `crates/git/src/diff/hunk_stage.rs`。
//! tag `v2-final`（`088b539bfa35e4466f8fe7c3550c730b74c551d4`）未包含当前目录布局；
//! 旧路径 `vcs/git/src/stage.rs` 与该基线逐字节相同，
//! `vcs/git/src/diff/hunk_stage.rs` 仅有 import 迁移差异。
//!
//! [`FileStatus`] 是唯一的文件状态类型（porcelain 九态映射）。
//! crate root 与 [`diff::FileStatus`] 为同一类型；[`DiffFile::status`] 使用该类型。

pub mod diff;
pub mod error;
pub mod process;
pub mod repo;
pub mod status;
pub mod worktree;

pub use diff::{
    paginate, DiffFile, DiffHunk, DiffLine, DiffOptions, DiffPage, DiffService, HunkId, LineKind,
};
pub use error::GitError;
pub use process::{validate_position_arg, GitRunner};
pub use repo::{GitService, Head, RepoInfo};
pub use status::{read_status, FileChange, FileStatus, StatusService, StatusSnapshot};
pub use worktree::{Worktree, WorktreeService};
