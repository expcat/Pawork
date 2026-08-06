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
//! - [`worktree`]：[`worktree::WorktreeService`] Worktree 创建/删除（清理不删用户数据）。
//! - [`cache`]：[`cache::StatusCache`] / [`cache::CachedStatusService`] 缓存与 watcher 失效。

pub mod cache;
pub mod error;
pub mod process;
pub mod repo;
pub mod stage;
pub mod status;
pub mod worktree;

pub use cache::{spawn_invalidator, CacheScope, CachedStatusService, StatusCache, WatcherGuard};
pub use error::GitError;
pub use process::GitRunner;
pub use repo::{GitService, Head, RepoInfo};
pub use stage::{StageOp, StageRequest, StageRisk, StageService};
pub use status::{read_status, FileChange, FileStatus, StatusService, StatusSnapshot};
pub use worktree::{Worktree, WorktreeService};
