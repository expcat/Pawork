use super::GuiHostAdapter;
use pawork_gui_server::GuiHostError;

pub(super) mod approval;
pub(super) mod attachments;
pub(super) mod command;
pub(super) mod computer;
pub(super) mod files;
pub(super) mod goal;
pub(super) mod mcp;
pub(super) mod plan;
pub(super) mod query;
pub(super) mod recording;
pub(super) mod run_start;
pub(super) mod session;
pub(super) mod settings;
pub(super) mod subagents;
pub(super) mod terminal;

/// Global 配置路径。`global_config_path` 为 None 表示无法解析当前用户的
/// Global 配置目录：Unix 取决于用户主目录；Windows 上 LocalAppData 与
/// RoamingAppData 任一失败即 None，并不查询主目录。写盘前 fail-closed，不改内存。
pub(super) fn global_config_file() -> Result<std::path::PathBuf, GuiHostError> {
    pawork_workspace::config::global_config_path().ok_or_else(|| {
        GuiHostAdapter::host_error(
            "config_unavailable",
            "无法解析当前用户的 Global 配置目录；请检查用户主目录及系统应用数据目录配置后重试",
        )
    })
}
