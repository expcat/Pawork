//! pawork-tools：只读四工具 + 写三件 + run_command + 接 PolicyEngine 的最小 scheduler。
//!
//! tool_search 与 MCP 客户端不在本包。

mod apply_patch;
mod common;
mod computer;
mod computer_native;
mod edit_file;
mod find_files;
mod list_directory;
mod read_file;
mod run_command;
mod scheduler;
mod search_text;
mod write_file;

pub use apply_patch::ApplyPatchTool;
pub use computer::ComputerTool;
pub use computer_native::NativeComputerTool;
pub use edit_file::EditFileTool;
pub use find_files::FindFilesTool;
pub use list_directory::ListDirectoryTool;
pub use read_file::ReadFileTool;
pub use run_command::RunCommandTool;
pub use scheduler::{
    ApprovalOutcome, ApprovalResolver, AutoApproveResolver, NoopToolEventSink, ToolRegistry,
    ToolRegistryError, ToolScheduler, ToolSchedulerConfig,
};
pub use search_text::SearchTextTool;
pub use write_file::WriteFileTool;
