use pawork_exec::PtyWindowSize;
use pawork_protocol::{AppCommand, AppCommandEnvelope, AppQuery, AppResponse, TerminalSettingsData};

use crate::gui_host::GuiHostAdapter;
use crate::gui_server::GuiHostError;

use super::settings_data;

/// ADR-050 D2：终端默认设置生效值。`shell` 为 Global 持久值（null =
/// 跟随平台默认），columns/rows 未设回落 exec 既有默认（`PtyWindowSize::
/// default()` = 80×24，单一事实源）。
pub(crate) async fn terminal_settings(
    adapter: &GuiHostAdapter,
    query: &AppQuery,
) -> Result<AppResponse, GuiHostError> {
    let AppQuery::TerminalSettings = query else {
        unreachable!("terminal_settings handler receives TerminalSettings")
    };
    let core = adapter.core.read().await;
    let defaults = PtyWindowSize::default();
    let terminal = core.config().terminal.as_ref();
    Ok(settings_data(TerminalSettingsData {
        shell: terminal.and_then(|config| config.shell.clone()),
        columns: terminal
            .and_then(|config| config.columns)
            .unwrap_or(defaults.cols),
        rows: terminal
            .and_then(|config| config.rows)
            .unwrap_or(defaults.rows),
    }))
}

/// 校验终端 shell（ADR-050 D3，fail-closed）：trim 后非空；含路径分隔符时
/// 路径必须存在，否则须在 PATH 中解析到文件。
fn validate_terminal_shell(shell: &str) -> Result<(), String> {
    if shell.is_empty() {
        return Err("shell must not be empty".to_string());
    }
    let path_like = shell.contains('/') || (cfg!(windows) && shell.contains('\\'));
    if path_like {
        if std::path::Path::new(shell).exists() {
            Ok(())
        } else {
            Err(format!("shell path {shell:?} does not exist"))
        }
    } else if std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .any(|dir| dir.join(shell).is_file())
    {
        Ok(())
    } else {
        Err(format!("shell {shell:?} not found in PATH"))
    }
}

/// ADR-050 D3：终端默认设置全态写。校验 fail-closed（shell trim 非空且
/// 可解析、columns/rows ∈ 2..=1000，非法即 Error 保旧）→ `write_terminal_settings`
/// Global 原子写 → 写锁内 `AppCore::set_terminal_settings` 同步内存；
/// 回执 Data 携带写入后的完整 TerminalSettings 形状。
pub(crate) async fn set_terminal_settings(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::SetTerminalSettings {
        shell,
        columns,
        rows,
    } = command
    else {
        unreachable!("set_terminal_settings handler receives SetTerminalSettings")
    };
    let shell = shell.as_deref().map(str::trim);
    if let Some(shell) = shell {
        validate_terminal_shell(shell)
            .map_err(|reason| GuiHostAdapter::host_error("invalid_terminal_settings", reason))?;
    }
    if !(2..=1000).contains(columns) || !(2..=1000).contains(rows) {
        return Err(GuiHostAdapter::host_error(
            "invalid_terminal_settings",
            format!("terminal size must be within 2..=1000 (got columns={columns}, rows={rows})"),
        ));
    }
    let path = pawork_workspace::config::global_config_path().ok_or_else(|| {
        GuiHostAdapter::host_error(
            "config_unavailable",
            "global config directory is not available on this platform",
        )
    })?;
    pawork_workspace::config::write_terminal_settings(&path, shell, *columns, *rows)
        .map_err(|error| GuiHostAdapter::host_error("config_write", error.to_string()))?;
    {
        let mut core = adapter.core.write().await;
        core.set_terminal_settings(shell.map(str::to_string), *columns, *rows);
    }
    Ok(settings_data(TerminalSettingsData {
        shell: shell.map(str::to_string),
        columns: *columns,
        rows: *rows,
    }))
}
