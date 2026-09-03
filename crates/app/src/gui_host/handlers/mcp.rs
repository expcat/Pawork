//! SET-6c 工具与 MCP 页（ADR-049）：McpTest / McpServerRemove Host 门面。
//!
//! Secret 红线：remove 只按配置中的 SecretRef 定位删除，绝不触碰明文；
//! 命名空间前缀 fail-closed（非 `pawork.mcp.*` 一律拒绝，先于写盘）。

use pawork_protocol::{AppCommand, AppCommandEnvelope, AppResponse};
use serde_json::json;

use crate::gui_server::GuiHostError;

use super::super::GuiHostAdapter;

/// ADR-049 D1：现场验证单个 MCP server（复用 `AppCore::mcp_test`：
/// ping + list_tools 并回写 slot 状态）。未知 server fail-closed（Error，
/// 不动现有 slot）；回执与 mcp_list 同形状的完整 servers 数组。
pub(crate) async fn mcp_test(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::McpTest { name } = command else {
        unreachable!("mcp_test handler receives McpTest")
    };
    {
        let core = adapter.core.read().await;
        let config = crate::extensions::mcp_config_from_pawork(core.config())
            .map_err(|error| GuiHostAdapter::host_error("mcp_config", error.to_string()))?;
        if config.server(name).is_none() {
            return Err(GuiHostAdapter::host_error(
                "unknown_mcp_server",
                format!("MCP server '{name}' is not configured"),
            ));
        }
    }
    let servers = {
        let mut core = adapter.core.write().await;
        core.mcp_test(Some(name))
            .await
            .map_err(GuiHostAdapter::app_error)?
    };
    let servers = serde_json::to_value(servers)
        .map_err(|error| GuiHostAdapter::host_error("internal", error.to_string()))?;
    Ok(AppResponse::Data(json!({ "servers": servers })))
}

/// ADR-049 D2：从 Global 层移除单个 MCP server。定序：校验 server 存在
/// 于合并配置 → SecretRef 命名空间预校验 → 盘上跨层同名探测（同名 server
/// 还定义在 Global 之外的层即 Error，先于任何写盘）→ Global 原子写 → 清理
/// `pawork.mcp.<name>` 下 SecretRef（幂等）→ 内存同步（shutdown slot
/// client → 删 slot → 重建 registry）。任一阶段失败即 Error 且如实回执；
/// name 不存在或跨层同名时盘/密/内存三处皆不动（fail-closed 保旧）。
pub(crate) async fn mcp_server_remove(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::McpServerRemove { name } = command else {
        unreachable!("mcp_server_remove handler receives McpServerRemove")
    };
    let (secret_refs, workspace_root) = {
        let core = adapter.core.read().await;
        let config = crate::extensions::mcp_config_from_pawork(core.config())
            .map_err(|error| GuiHostAdapter::host_error("mcp_config", error.to_string()))?;
        let Some(server) = config.server(name) else {
            return Err(GuiHostAdapter::host_error(
                "unknown_mcp_server",
                format!("MCP server '{name}' is not configured"),
            ));
        };
        let secret_refs = crate::extensions::mcp_server_secrets_for_removal(name, server)
            .map_err(GuiHostAdapter::app_error)?;
        let workspace_root = core.workspace_root().map(|root| root.to_path_buf());
        (secret_refs, workspace_root)
    };
    let path = pawork_workspace::config::global_config_path().ok_or_else(|| {
        GuiHostAdapter::host_error(
            "config_unavailable",
            "global config directory is not available on this platform",
        )
    })?;
    // P2 跨层同名守卫：按盘上配置探测同名 server 是否还定义在 Global 之外
    // 的层（workspace / 派生 profile 等）。仅删 Global 条目会让它在下次装配
    // 时复活，而其 SecretRef 已被清理；故写盘前命中即 fail-closed 拒绝。
    let outside_layers = mcp_server_layers_defining_outside_global(workspace_root.as_deref(), name)
        .map_err(|error| GuiHostAdapter::host_error("config_probe", error.to_string()))?;
    if !outside_layers.is_empty() {
        return Err(GuiHostAdapter::host_error(
            "mcp_server_defined_in_other_layers",
            format!(
                "MCP server '{name}' is also defined in other config layers ({}); \
                 remove those definitions first (disk, secrets and in-memory state \
                 are unchanged)",
                outside_layers.join(", ")
            ),
        ));
    }
    let removed = pawork_workspace::config::write_mcp_server_remove(&path, name)
        .map_err(|error| GuiHostAdapter::host_error("config_write", error.to_string()))?;
    if !removed {
        return Err(GuiHostAdapter::host_error(
            "mcp_server_not_global",
            format!("MCP server '{name}' is not configured in the global layer"),
        ));
    }
    // 写盘已成功：后续清密失败仍要把内存同步到盘，避免 UI/内存继续
    // 展示已删除的 server。清密错误在内存同步后再如实回执。
    let secret_error = crate::extensions::clear_mcp_server_secrets(&secret_refs)
        .err()
        .map(|error| GuiHostAdapter::host_error("secret_cleanup", error.to_string()));
    let servers = {
        let mut core = adapter.core.write().await;
        core.remove_mcp_server(name)
            .await
            .map_err(GuiHostAdapter::app_error)?;
        core.mcp_list()
    };
    if let Some(error) = secret_error {
        return Err(error);
    }
    let servers = serde_json::to_value(servers)
        .map_err(|error| GuiHostAdapter::host_error("internal", error.to_string()))?;
    Ok(AppResponse::Data(json!({ "servers": servers })))
}

/// 盘上跨层同名探测：重放 `Loader::discover`，返回仍定义 `mcp.servers.<name>`
/// 的非 Builtin/Global 层名（去重）。探测以盘上配置为准，而非内存合并视图。
fn mcp_server_layers_defining_outside_global(
    workspace_root: Option<&std::path::Path>,
    name: &str,
) -> Result<Vec<&'static str>, pawork_workspace::config::ConfigError> {
    use pawork_workspace::config::ConfigTier;

    let resolved = pawork_workspace::config::Loader::discover(workspace_root).resolve()?;
    let mut tiers: Vec<&'static str> = Vec::new();
    for source in &resolved.sources {
        if matches!(source.span.tier, ConfigTier::Builtin | ConfigTier::Global) {
            continue;
        }
        let defined = source
            .value
            .as_value()
            .get("mcp")
            .and_then(|mcp| mcp.get("servers"))
            .and_then(|servers| servers.get(name))
            .is_some();
        if defined {
            let tier = source.span.tier.as_str();
            if !tiers.contains(&tier) {
                tiers.push(tier);
            }
        }
    }
    Ok(tiers)
}
