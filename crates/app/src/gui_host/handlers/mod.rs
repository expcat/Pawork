use super::GuiHostAdapter;
use pawork_gui_server::GuiHostError;

pub(super) mod approval;
pub(super) mod attachments;
pub(super) mod command;
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

/// Global 配置路径（不可用即 `config_unavailable`）。
pub(super) fn global_config_file() -> Result<std::path::PathBuf, GuiHostError> {
    pawork_workspace::config::global_config_path().ok_or_else(|| {
        GuiHostAdapter::host_error(
            "config_unavailable",
            "global config directory is not available on this platform",
        )
    })
}
