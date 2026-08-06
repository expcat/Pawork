//! Pawork 系统 Git 封装（Phase 7）。
//!
//! 本 crate 以系统 `git`（ADR-007）为唯一后端，统一经
//! [`process_runtime::ProcessRuntime`] 执行命令，并把 `ProcessError`、非零退出、
//! 超时与取消归一为 [`GitError`]。
//!
//! 模块：
//! - [`error`]：错误归一。
//! - [`process`]：[`GitRunner`] 系统 git 统一调用入口。
//! - [`repo`]：[`GitService`] 仓库检测与 HEAD/branch 元信息。
//! - [`status`]：[`status::StatusService`] 工作区状态解析。
//! - [`stage`]：[`stage::StageService`] stage / unstage / discard。
//! - [`commit`]：[`commit::CommitService`] commit（含 amend / allow-empty）。
//! - [`branch`]：[`branch::BranchService`] branch 创建/删除与 checkout。
//! - [`stash`]：[`stash::StashService`] stash push/list/pop/apply/drop。
//! - [`history`]：[`history::HistoryService`] log / show / merge-base。
//! - [`conflict`]：[`conflict::ConflictService`] 未合并路径与 merge 状态。
//! - [`worktree`]：[`worktree::WorktreeService`] Worktree 创建/删除（清理不删用户数据）。
//! - [`cache`]：[`cache::StatusCache`] / [`cache::CachedStatusService`] 缓存与 watcher 失效。

pub mod branch;
pub mod cache;
pub mod commit;
pub mod conflict;
pub mod error;
pub mod history;
pub mod process;
pub mod repo;
pub mod stage;
pub mod stash;
pub mod status;
pub mod worktree;

pub use branch::BranchService;
pub use cache::{spawn_invalidator, CacheScope, CachedStatusService, StatusCache, WatcherGuard};
pub use commit::{CommitOptions, CommitService};
pub use conflict::{ConflictService, UnmergedEntry};
pub use error::GitError;
pub use history::{CommitDetail, CommitInfo, HistoryService, LogOptions};
pub use process::GitRunner;
pub use repo::{GitService, Head, RepoInfo};
pub use stage::{StageOp, StageRequest, StageRisk, StageService};
pub use stash::{StashEntry, StashPushOutcome, StashService};
pub use status::{read_status, FileChange, FileStatus, StatusService, StatusSnapshot};
pub use worktree::{Worktree, WorktreeService};
