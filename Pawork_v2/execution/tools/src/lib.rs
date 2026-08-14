//! pawork-tools：只读四工具 + 写三件 + 接 PolicyEngine 的最小 scheduler。
//!
//! run_command / tool_search 仍不在本包。

pub mod apply_patch;
pub mod common;
pub mod edit_file;
pub mod find_files;
pub mod list_directory;
pub mod read_file;
pub mod scheduler;
pub mod search_text;
pub mod write_file;

pub use apply_patch::ApplyPatchTool;
pub use edit_file::EditFileTool;
pub use find_files::FindFilesTool;
pub use list_directory::ListDirectoryTool;
pub use read_file::ReadFileTool;
pub use scheduler::{
    ApprovalOutcome, ApprovalResolver, AutoApproveResolver, NoopToolEventSink, ToolRegistry,
    ToolRegistryError, ToolScheduler, ToolSchedulerConfig,
};
pub use search_text::SearchTextTool;
pub use write_file::WriteFileTool;
