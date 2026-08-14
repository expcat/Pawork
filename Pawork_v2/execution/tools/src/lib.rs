//! pawork-tools：只读四工具 + 最小 scheduler（S2 波 B）。
//!
//! 写工具 / run_command / tool_search 不在本包本阶段。

pub mod common;
pub mod find_files;
pub mod list_directory;
pub mod read_file;
pub mod scheduler;
pub mod search_text;

pub use find_files::FindFilesTool;
pub use list_directory::ListDirectoryTool;
pub use read_file::ReadFileTool;
pub use scheduler::{
    ApprovalOutcome, ApprovalResolver, AutoApproveResolver, NoopToolEventSink, ToolRegistry,
    ToolRegistryError, ToolScheduler, ToolSchedulerConfig,
};
pub use search_text::SearchTextTool;
