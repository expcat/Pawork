//! 结构化 Diff：unified parser、[`DiffService`]、[`HunkStageService`]。
//!
//! 把系统 git 的 diff 输出解析为结构化 [`model::DiffFile`] / [`model::DiffHunk`] /
//! [`model::DiffLine`]，支持 rename/binary/untracked/submodule、CRLF、无末尾换行、
//! Unicode 文件名，并可分页。HunkId 全局自增。
//!
//! - [`service::DiffService`]：调 git + 解析。
//! - [`parser`]：unified diff 状态机解析（纯内存，100k 行 < 500ms）。
//! - [`service::paginate`]：分页。
//! - [`hunk_stage::HunkStageService`]：基于结构化 Diff 的 hunk / line 级暂存
//!   与取消暂存。

pub mod hunk_stage;
pub mod model;
pub mod parser;
pub mod service;

pub use crate::status::FileStatus;
pub use hunk_stage::{build_hunk_patch, build_line_patch, HunkStageService};
pub use model::{DiffFile, DiffHunk, DiffLine, HunkId, LineKind};
pub use parser::parse_unified;
pub use service::{paginate, DiffOptions, DiffPage, DiffService};
