//! GUI Command / Query 构造与回执解析。
//!
//! JSON 键与字段名保持桌面侧既有拼装。

use pawork_client::{
    AppCommand, AppQuery, AppResponse, AppResponseEnvelope, ApprovalModeWire, AuthStartData,
    ClientAuthentication, DefaultModelPair, GeneralSettingsData, PermissionsSettingsData,
    ProviderAuthStatusData, ProviderUseProxyData, SubagentListData, SubagentSettingsData,
    TerminalSettingsData, TimelinePage, TOKEN_SCHEME,
};
use serde_json::json;

use crate::projection::{ModelEntry, SettingsRole};

use super::{
    ComposerOptions, DiffFileDetail, DiffFileSummary, DiffHunkDetail, DiffLineDetail, DiffLineKind,
    GitDiffInfo, McpServerEntry, PAGE_LIMIT,
};

/// WorkspaceId / SessionId 等 domain id 未从 pawork-client re-export；命令与
/// 查询经冻结的 serde 形状（method/params）构造，避免引入第二个业务依赖。
/// ADR-054 D1（since 1.11）：workspace_id = None 时缺省该字段 → Host 落盘
/// 无归属会话；显式传值行为不变。
pub(super) fn session_create_command(workspace_id: Option<&str>) -> AppCommand {
    let params = match workspace_id {
        Some(workspace_id) => json!({ "workspace_id": workspace_id }),
        None => json!({}),
    };
    serde_json::from_value(json!({
        "method": "session_create",
        "params": params
    }))
    .expect("session_create command shape is frozen")
}

/// ADR-054 D2：会话改名。title trim 后为空由 Host 结构化拒绝，不写盘。
pub(super) fn session_rename_command(session_id: &str, title: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "session_rename",
        "params": { "session_id": session_id, "title": title }
    }))
    .expect("session_rename command shape is frozen")
}

/// ADR-054 D3：归档 / 反归档。Desktop 只暴露归档入口（archived = true）。
pub(super) fn session_archive_command(session_id: &str, archived: bool) -> AppCommand {
    serde_json::from_value(json!({
        "method": "session_archive",
        "params": { "session_id": session_id, "archived": archived }
    }))
    .expect("session_archive command shape is frozen")
}

pub(super) fn workspace_add_command(root_path: &std::path::Path) -> AppCommand {
    serde_json::from_value(json!({
        "method": "workspace_add",
        "params": { "root_path": root_path.to_string_lossy() }
    }))
    .expect("workspace_add command shape is frozen")
}

pub(super) fn session_fork_command(session_id: &str, parent_event_id: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "session_fork",
        "params": {
            "session_id": session_id,
            "parent_event_id": parent_event_id
        }
    }))
    .expect("session_fork command shape is frozen")
}

pub(super) fn is_workspace_relative_cwd(cwd: &str) -> bool {
    let trimmed = cwd.trim();
    if trimmed.is_empty() {
        return false;
    }
    let bytes = trimmed.as_bytes();
    let has_windows_prefix = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    !(trimmed.starts_with(['/', '\\'])
        || has_windows_prefix
        || trimmed
            .split(['/', '\\'])
            .any(|component| component == ".."))
}

pub(super) fn terminal_create_command(
    workspace_id: &str,
    cwd: Option<&str>,
) -> Result<AppCommand, String> {
    let mut params = json!({ "workspace_id": workspace_id });
    if let Some(cwd) = cwd.map(str::trim).filter(|cwd| !cwd.is_empty()) {
        if !is_workspace_relative_cwd(cwd) {
            return Err("cwd must be a workspace-relative path".into());
        }
        params["working_directory"] = json!(cwd);
    }
    serde_json::from_value(json!({
        "method": "terminal_create",
        "params": params
    }))
    .map_err(|error| format!("terminal_create command shape: {error}"))
}

pub(super) fn terminal_write_command(terminal_session_id: &str, data: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "terminal_write",
        "params": {
            "terminal_session_id": terminal_session_id,
            "data": data
        }
    }))
    .expect("terminal_write command shape is frozen")
}

pub(super) fn terminal_resize_command(
    terminal_session_id: &str,
    columns: u16,
    rows: u16,
) -> AppCommand {
    serde_json::from_value(json!({
        "method": "terminal_resize",
        "params": {
            "terminal_session_id": terminal_session_id,
            "columns": columns,
            "rows": rows
        }
    }))
    .expect("terminal_resize command shape is frozen")
}

pub(super) fn terminal_close_command(terminal_session_id: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "terminal_close",
        "params": {
            "terminal_session_id": terminal_session_id
        }
    }))
    .expect("terminal_close command shape is frozen")
}

pub(super) fn auth_start_command(provider_id: &str, flow: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "auth_start",
        "params": { "provider_id": provider_id, "flow": flow }
    }))
    .expect("auth_start command shape is frozen")
}

/// ApiKeySecret 在 wire 上是透明字符串；明文只在本函数栈上的 Value 里
/// 短暂停留，from_value 后即弃，不落任何字段或日志。
pub(super) fn auth_set_api_key_command(provider_id: &str, api_key: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "auth_set_api_key",
        "params": { "provider_id": provider_id, "api_key": api_key }
    }))
    .expect("auth_set_api_key command shape is frozen")
}

pub(super) fn auth_cancel_command(provider_id: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "auth_cancel",
        "params": { "provider_id": provider_id }
    }))
    .expect("auth_cancel command shape is frozen")
}

pub(super) fn auth_remove_command(provider_id: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "auth_remove",
        "params": { "provider_id": provider_id }
    }))
    .expect("auth_remove command shape is frozen")
}

pub(super) fn set_default_role_model_command(
    role: &str,
    value: Option<&DefaultModelPair>,
) -> AppCommand {
    serde_json::from_value(json!({
        "method": "set_default_role_model",
        "params": { "role": role, "value": value }
    }))
    .expect("set_default_role_model command shape is frozen")
}

pub(super) fn forked_session_id(response: &AppResponseEnvelope) -> Option<String> {
    match &response.response {
        AppResponse::Data(data) => data
            .get("session_id")
            .or_else(|| data.get("branch_id"))
            .and_then(|value| value.as_str())
            .map(str::to_string),
        _ => None,
    }
}

pub(super) fn workspace_opened(response: &AppResponseEnvelope) -> Option<(String, String)> {
    match &response.response {
        AppResponse::Data(data) => Some((
            data.get("id")?.as_str()?.to_string(),
            data.get("name")?.as_str()?.to_string(),
        )),
        _ => None,
    }
}

/// 只接受 Data 里的 terminal_session_id 字符串。
/// 身份键只有它。Host 与 golden 不发 id，协商 minor 也不改键。
pub(super) fn terminal_session_id(response: &AppResponseEnvelope) -> Option<String> {
    match &response.response {
        AppResponse::Data(data) => data
            .get("terminal_session_id")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        _ => None,
    }
}

pub(super) fn session_get_query(session_id: &str, after: Option<u64>) -> AppQuery {
    serde_json::from_value(json!({
        "method": "session_get",
        "params": {
            "session_id": session_id,
            "timeline_after_sequence": after,
            "timeline_limit": PAGE_LIMIT
        }
    }))
    .expect("session_get query shape is frozen")
}

pub(super) fn load_desktop_authentication(
    token_path: &std::path::Path,
) -> Result<ClientAuthentication, String> {
    let bytes = std::fs::read(token_path).map_err(|error| {
        format!(
            "gui token file not found or unreadable ({}): {error}",
            token_path.display()
        )
    })?;
    let text = String::from_utf8(bytes).map_err(|_| {
        format!(
            "gui token file is empty or malformed: {}",
            token_path.display()
        )
    })?;
    let proof = text.trim();
    if proof.is_empty() {
        return Err(format!(
            "gui token file is empty or malformed: {}",
            token_path.display()
        ));
    }
    Ok(ClientAuthentication {
        scheme: TOKEN_SCHEME.into(),
        proof: proof.to_string(),
    })
}

pub(super) fn run_start_command(
    session_id: &str,
    text: &str,
    model: Option<&(String, String)>,
    effort: Option<&str>,
    options: &ComposerOptions,
) -> AppCommand {
    let mut params = json!({
        "session_id": session_id,
        "user_message": text
    });
    if let Some((provider, id)) = model {
        params["provider"] = json!(provider);
        params["model"] = json!(id);
    }
    // ADR-063（API 1.21）：显式 effort 优先于 Host 侧模型默认；None
    // 省略参数，Host 回落 [reasoning] 模型默认 → Provider 默认。
    if let Some(effort) = effort {
        params["effort"] = json!(effort);
    }
    if !options.attachments.is_empty() {
        params["attachment_ids"] = json!(options
            .attachments
            .iter()
            .map(|a| &a.id)
            .collect::<Vec<_>>());
    }
    if let Some(search) = options.web_search {
        params["web_search"] = json!(search);
    }
    if !options.video_urls.is_empty() {
        params["video_urls"] = json!(options.video_urls);
    }
    serde_json::from_value(json!({
        "method": "run_start",
        "params": params
    }))
    .expect("run_start command shape is frozen")
}

pub(super) fn run_cancel_command(run_id: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "run_cancel",
        "params": { "run_id": run_id }
    }))
    .expect("run_cancel command shape is frozen")
}

pub(super) fn tool_approve_command(run_id: &str, tool_call_id: &str, decision: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "tool_approve",
        "params": {
            "run_id": run_id,
            "tool_call_id": tool_call_id,
            "decision": decision
        }
    }))
    .expect("tool_approve command shape is frozen")
}

pub(super) fn model_list_query() -> AppQuery {
    serde_json::from_value(json!({
        "method": "model_list",
        "params": {}
    }))
    .expect("model_list query shape is frozen")
}

pub(super) fn model_catalog_query() -> AppQuery {
    serde_json::from_value(json!({
        "method": "model_list",
        "params": { "include_disabled": true }
    }))
    .expect("model_list include_disabled query shape is frozen")
}

pub(super) fn provider_auth_status_query() -> AppQuery {
    serde_json::from_value(json!({
        "method": "provider_auth_status",
        "params": {}
    }))
    .expect("provider_auth_status query shape is frozen")
}

pub(super) fn general_settings_query() -> AppQuery {
    serde_json::from_value(json!({
        "method": "general_settings"
    }))
    .expect("general_settings query shape is frozen")
}

pub(super) fn permissions_settings_query() -> AppQuery {
    serde_json::from_value(json!({
        "method": "permissions_settings"
    }))
    .expect("permissions_settings query shape is frozen")
}

pub(super) fn terminal_settings_query() -> AppQuery {
    serde_json::from_value(json!({
        "method": "terminal_settings"
    }))
    .expect("terminal_settings query shape is frozen")
}

pub(super) fn subagent_settings_query() -> AppQuery {
    serde_json::from_value(json!({
        "method": "subagent_settings"
    }))
    .expect("subagent_settings query shape is frozen")
}

pub(super) fn subagent_list_query(session_id: &str) -> AppQuery {
    serde_json::from_value(json!({
        "method": "subagent_list",
        "params": { "session_id": session_id }
    }))
    .expect("subagent_list query shape is frozen")
}

pub(super) fn set_subagent_settings_command(settings: &SubagentSettingsData) -> AppCommand {
    serde_json::from_value(json!({
        "method": "set_subagent_settings",
        "params": { "settings": settings }
    }))
    .expect("set_subagent_settings command shape is frozen")
}

pub(super) fn set_proxy_url_command(proxy_url: Option<&str>) -> AppCommand {
    serde_json::from_value(json!({
        "method": "set_proxy_url",
        "params": { "proxy_url": proxy_url }
    }))
    .expect("set_proxy_url command shape is frozen")
}

pub(super) fn set_provider_use_proxy_command(provider_id: &str, use_proxy: bool) -> AppCommand {
    serde_json::from_value(json!({
        "method": "set_provider_use_proxy",
        "params": { "provider_id": provider_id, "use_proxy": use_proxy }
    }))
    .expect("set_provider_use_proxy command shape is frozen")
}

pub(super) fn set_model_enabled_command(
    provider_id: &str,
    model_id: &str,
    enabled: bool,
) -> AppCommand {
    serde_json::from_value(json!({
        "method": "set_model_enabled",
        "params": { "provider_id": provider_id, "model_id": model_id, "enabled": enabled }
    }))
    .expect("set_model_enabled command shape is frozen")
}

/// set_model_reasoning（ADR-063 / API 1.21）：全态写单模型的推理强度
/// 偏好——default_effort 为默认强度，supported_efforts 为手动范围声明；
/// 两者皆 None = 删除该模型条目（回落目录 / Provider 默认）。
pub(super) fn set_model_reasoning_command(
    provider_id: &str,
    model_id: &str,
    default_effort: Option<&str>,
    supported_efforts: Option<&[String]>,
) -> AppCommand {
    serde_json::from_value(json!({
        "method": "set_model_reasoning",
        "params": {
            "provider_id": provider_id,
            "model_id": model_id,
            "default_effort": default_effort,
            "supported_efforts": supported_efforts,
        }
    }))
    .expect("set_model_reasoning command shape is frozen")
}

pub(super) fn set_provider_models_enabled_command(provider_id: &str, enabled: bool) -> AppCommand {
    serde_json::from_value(json!({
        "method": "set_provider_models_enabled",
        "params": { "provider_id": provider_id, "enabled": enabled }
    }))
    .expect("set_provider_models_enabled command shape is frozen")
}

pub(super) fn set_approval_mode_command(mode: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "set_approval_mode",
        "params": { "mode": mode }
    }))
    .expect("set_approval_mode command shape is frozen")
}

pub(super) fn set_workspace_trust_command(workspace_id: &str, trusted: bool) -> AppCommand {
    serde_json::from_value(json!({
        "method": "workspace_trust",
        "params": { "workspace_id": workspace_id, "trusted": trusted }
    }))
    .expect("workspace_trust command shape is frozen")
}

pub(super) fn set_terminal_settings_command(
    shell: Option<&str>,
    columns: u16,
    rows: u16,
) -> AppCommand {
    serde_json::from_value(json!({
        "method": "set_terminal_settings",
        "params": { "shell": shell, "columns": columns, "rows": rows }
    }))
    .expect("set_terminal_settings command shape is frozen")
}

pub(super) fn diff_list_files_query(workspace_id: &str) -> AppQuery {
    serde_json::from_value(json!({
        "method": "diff_list_files",
        "params": { "workspace_id": workspace_id }
    }))
    .expect("diff_list_files query shape is frozen")
}

pub(super) fn diff_get_query(workspace_id: &str, path: &str) -> AppQuery {
    serde_json::from_value(json!({
        "method": "diff_get",
        "params": {
            "workspace_id": workspace_id,
            "path": path
        }
    }))
    .expect("diff_get query shape is frozen")
}

pub(super) fn mcp_list_query() -> AppQuery {
    serde_json::from_value(json!({
        "method": "mcp_list"
    }))
    .expect("mcp_list query shape is frozen")
}

pub(super) fn mcp_test_command(name: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "mcp_test",
        "params": { "name": name }
    }))
    .expect("mcp_test command shape is frozen")
}

pub(super) fn mcp_server_remove_command(name: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "mcp_server_remove",
        "params": { "name": name }
    }))
    .expect("mcp_server_remove command shape is frozen")
}

pub(super) fn parse_models(response: &AppResponseEnvelope) -> Result<Vec<ModelEntry>, String> {
    match &response.response {
        AppResponse::Data(data) => {
            let entries = data
                .as_array()
                .ok_or_else(|| "model list is not an array".to_string())?;
            Ok(entries
                .iter()
                .filter_map(|entry| {
                    let provider_id = entry.get("provider_id").and_then(|value| value.as_str())?;
                    let id = entry.get("id").and_then(|value| value.as_str())?;
                    let display_name = entry
                        .get("display_name")
                        .and_then(|value| value.as_str())
                        .unwrap_or(id);
                    // ADR-055 D4：响应条目 additive 增 enabled；旧 Host
                    // 缺字段即视为启用（缺省口径只回启用模型）。
                    let enabled = entry
                        .get("enabled")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(true);
                    // ADR-063（API 1.21）：additive 能力位与推理强度词汇；
                    // 旧 Host 缺字段 = 无能力徽标 / 范围未知 / 未配置默认。
                    let string_list = |key: &str| {
                        entry.get(key).and_then(|value| {
                            value.as_array().map(|items| {
                                items
                                    .iter()
                                    .filter_map(|item| item.as_str().map(str::to_string))
                                    .collect::<Vec<_>>()
                            })
                        })
                    };
                    Some(ModelEntry {
                        provider_id: provider_id.to_string(),
                        id: id.to_string(),
                        display_name: display_name.to_string(),
                        context_window_tokens: entry
                            .get("context_window_tokens")
                            .and_then(serde_json::Value::as_u64),
                        enabled,
                        image_input: entry
                            .get("image_input")
                            .and_then(|value| value.as_bool())
                            .unwrap_or(false),
                        // ADR-064（API 1.25）：additive；旧 Host 缺字段 =
                        // 无生图徽标 / 筛选不可命中。
                        image_output: entry
                            .get("image_output")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                        // ADR-064（API 1.25）：旧 Host 目录只含对话模型，
                        // text 缺字段 = true。
                        text: entry
                            .get("text")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(true),
                        video_input: entry
                            .get("video_input")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                        web_search: entry
                            .get("web_search")
                            .and_then(|value| value.as_bool())
                            .unwrap_or(false),
                        catalog_efforts: string_list("catalog_efforts"),
                        default_effort: entry
                            .get("default_effort")
                            .and_then(|value| value.as_str())
                            .map(str::to_string),
                        manual_efforts: string_list("manual_efforts"),
                    })
                })
                .collect())
        }
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// set_model_enabled 回执的 Desktop 侧形状（ADR-055 D2；wire 字段同源，
/// 手工解包与 parse_models 同先例——pawork-client 不再导出 protocol
/// Data 类型时不引入第二业务依赖）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ModelEnabledReceipt {
    pub provider_id: String,
    pub model_id: String,
    pub enabled: bool,
    pub cleared_roles: Vec<String>,
}

/// set_provider_models_enabled 回执的 Desktop 侧形状（ADR-055 D2）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProviderModelsEnabledReceipt {
    pub provider_id: String,
    pub enabled: bool,
    pub cleared_roles: Vec<String>,
}

/// set_model_reasoning 回执的 Desktop 侧形状（ADR-063 / API 1.21）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ModelReasoningReceipt {
    pub provider_id: String,
    pub model_id: String,
    pub default_effort: Option<String>,
    pub manual_efforts: Option<Vec<String>>,
}

/// 解包 set_model_reasoning 信封：Data 为 `{ provider_id, model_id,
/// default_effort, supported_efforts }`（回执即写后状态）。
pub(super) fn parse_model_reasoning_confirmation(
    response: &AppResponseEnvelope,
) -> Result<ModelReasoningReceipt, String> {
    let data = match &response.response {
        AppResponse::Data(data) => data,
        AppResponse::Error(error) => return Err(error.message.clone()),
        other => return Err(format!("unexpected response: {other:?}")),
    };
    let object = data
        .as_object()
        .ok_or_else(|| "model reasoning receipt is not an object".to_string())?;
    let field = |key: &str| {
        object
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    let manual_efforts = object.get("supported_efforts").and_then(|value| {
        value.as_array().map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
    });
    Ok(ModelReasoningReceipt {
        provider_id: field("provider_id").ok_or("missing provider_id")?,
        model_id: field("model_id").ok_or("missing model_id")?,
        default_effort: field("default_effort"),
        manual_efforts,
    })
}

/// 解包 set_model_enabled 信封：Data 为 `{ provider_id, model_id,
/// enabled, cleared_roles }`；Error 取 Host 脱敏 message 原文。
pub(super) fn parse_model_enabled_confirmation(
    response: &AppResponseEnvelope,
) -> Result<ModelEnabledReceipt, String> {
    let data = match &response.response {
        AppResponse::Data(data) => data,
        AppResponse::Error(error) => return Err(error.message.clone()),
        other => return Err(format!("unexpected response: {other:?}")),
    };
    let object = data
        .as_object()
        .ok_or_else(|| "model enabled receipt is not an object".to_string())?;
    let field = |key: &str| {
        object
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    Ok(ModelEnabledReceipt {
        provider_id: field("provider_id").ok_or("missing provider_id")?,
        model_id: field("model_id").ok_or("missing model_id")?,
        enabled: object
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .ok_or("missing enabled")?,
        cleared_roles: parse_cleared_roles(object),
    })
}

/// 解包 set_provider_models_enabled 信封：Data 为 `{ provider_id,
/// enabled, cleared_roles }`。
pub(super) fn parse_provider_models_enabled_confirmation(
    response: &AppResponseEnvelope,
) -> Result<ProviderModelsEnabledReceipt, String> {
    let data = match &response.response {
        AppResponse::Data(data) => data,
        AppResponse::Error(error) => return Err(error.message.clone()),
        other => return Err(format!("unexpected response: {other:?}")),
    };
    let object = data
        .as_object()
        .ok_or_else(|| "provider models receipt is not an object".to_string())?;
    let field = |key: &str| {
        object
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    Ok(ProviderModelsEnabledReceipt {
        provider_id: field("provider_id").ok_or("missing provider_id")?,
        enabled: object
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .ok_or("missing enabled")?,
        cleared_roles: parse_cleared_roles(object),
    })
}

/// cleared_roles 按 wire 名原样保留（角色显示名由 UI 层映射，未知值不
/// 臆造）；畸形条目剔除，缺键视为空（启用路径恒为空）。
fn parse_cleared_roles(object: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
    object
        .get("cleared_roles")
        .and_then(serde_json::Value::as_array)
        .map(|roles| {
            roles
                .iter()
                .filter_map(|role| role.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// 解包 provider_auth_status 信封：`AppResponse::Data` 载荷形如
/// `{"providers":[…]}`，条目解析在 projection（纯状态可单测）。
pub(super) fn parse_provider_status_response(
    response: &AppResponseEnvelope,
) -> Result<ProviderAuthStatusData, String> {
    match &response.response {
        AppResponse::Data(data) => {
            serde_json::from_value(data.clone()).map_err(|error| error.to_string())
        }
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 general_settings / set_proxy_url 信封：Data 为
/// `{ "proxy_url": string | null }`；Error 取 Host 脱敏 message 原文
///（不含 proxy URL）。
pub(super) fn parse_general_settings_response(
    response: &AppResponseEnvelope,
) -> Result<GeneralSettingsData, String> {
    match &response.response {
        AppResponse::Data(data) => {
            serde_json::from_value(data.clone()).map_err(|error| error.to_string())
        }
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 set_provider_use_proxy 信封：Data 为
/// `{ "provider_id": string, "use_proxy": bool }`；Error 取 Host 脱敏
/// message 原文。
pub(super) fn parse_provider_use_proxy_response(
    response: &AppResponseEnvelope,
) -> Result<ProviderUseProxyData, String> {
    match &response.response {
        AppResponse::Data(data) => {
            serde_json::from_value(data.clone()).map_err(|error| error.to_string())
        }
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 permissions_settings 信封（SET-6b）：Data 为
/// `{ approval_mode, workspace_trusted, trust_workspaces_global }`；Error
/// 取 Host 脱敏 message 原文。
pub(super) fn parse_permissions_settings_response(
    response: &AppResponseEnvelope,
) -> Result<PermissionsSettingsData, String> {
    match &response.response {
        AppResponse::Data(data) => {
            serde_json::from_value(data.clone()).map_err(|error| error.to_string())
        }
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 terminal_settings / set_terminal_settings 信封（SET-6d /
/// ADR-050）：Data 为 `{ shell, columns, rows }`（查询与写回执同形状）；
/// Error 取 Host 脱敏 message 原文。
pub(super) fn parse_terminal_settings_response(
    response: &AppResponseEnvelope,
) -> Result<TerminalSettingsData, String> {
    match &response.response {
        AppResponse::Data(data) => {
            serde_json::from_value(data.clone()).map_err(|error| error.to_string())
        }
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 subagent_settings / set_subagent_settings 信封：Data 为协议
/// `SubagentSettingsData`（查询与写回执同形状）；Error 取 Host 脱敏
/// message 原文。
pub(super) fn parse_subagent_settings_response(
    response: &AppResponseEnvelope,
) -> Result<SubagentSettingsData, String> {
    match &response.response {
        AppResponse::Data(data) => {
            serde_json::from_value(data.clone()).map_err(|error| error.to_string())
        }
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 subagent_list 信封：Data 为协议 `SubagentListData`。
pub(super) fn parse_subagent_list_response(
    response: &AppResponseEnvelope,
) -> Result<SubagentListData, String> {
    match &response.response {
        AppResponse::Data(data) => {
            serde_json::from_value(data.clone()).map_err(|error| error.to_string())
        }
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 set_approval_mode 回执：Data 携带写后 `{ approval_mode }`；未知
/// 档位 fail-closed 报错（UI 保留旧值）。
pub(super) fn parse_approval_mode_confirmation(
    response: &AppResponseEnvelope,
) -> Result<ApprovalModeWire, String> {
    match &response.response {
        AppResponse::Data(data) => {
            let value = data
                .get("approval_mode")
                .cloned()
                .ok_or_else(|| "missing approval_mode".to_string())?;
            serde_json::from_value(value).map_err(|error| error.to_string())
        }
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 workspace_trust 回执：Data 携带写后 `{ workspace_trusted }`。
pub(super) fn parse_workspace_trust_confirmation(
    response: &AppResponseEnvelope,
) -> Result<bool, String> {
    match &response.response {
        AppResponse::Data(data) => data
            .get("workspace_trusted")
            .and_then(serde_json::Value::as_bool)
            .ok_or_else(|| "workspace trust receipt missing workspace_trusted".to_string()),
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 set_default_role_model 响应（OPT-3b / ADR-055 D5）：Data 携带
/// `{ role, value }`（value 必填可空，清除时为 null）；role 按已知 wire
/// 名 fail-closed（未知 echo 不落地任何状态）；Error 取 Host 脱敏 message。
pub(super) fn parse_default_role_model_confirmation(
    response: &AppResponseEnvelope,
) -> Result<(SettingsRole, Option<DefaultModelPair>), String> {
    match &response.response {
        AppResponse::Data(data) => {
            let role = data
                .get("role")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "set default role model receipt missing role".to_string())?;
            let role = SettingsRole::from_wire_name(role)
                .ok_or_else(|| format!("set default role model receipt unknown role {role}"))?;
            let value = data
                .get("value")
                .cloned()
                .ok_or_else(|| "set default role model receipt missing value".to_string())?;
            let value = serde_json::from_value::<Option<DefaultModelPair>>(value)
                .map_err(|error| error.to_string())?;
            Ok((role, value))
        }
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 auth_start 响应：verification_url 必填，user_code / expires_at
/// 仅 device flow 携带（PKCE 为 None）。
pub(super) fn parse_auth_started(response: &AppResponseEnvelope) -> Result<AuthStartData, String> {
    match &response.response {
        AppResponse::Data(data) => {
            serde_json::from_value(data.clone()).map_err(|error| error.to_string())
        }
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

pub(super) fn timeline_page(
    response: &AppResponseEnvelope,
) -> Result<Option<TimelinePage>, String> {
    match &response.response {
        AppResponse::Data(data) => match data.get("timeline_page") {
            Some(page) => serde_json::from_value::<TimelinePage>(page.clone())
                .map(Some)
                .map_err(|error| format!("decode timeline page: {error}")),
            None => Ok(None),
        },
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

pub(super) fn required_str(entry: &serde_json::Value, field: &str) -> Result<String, String> {
    entry
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("entry missing {field}"))
}

pub(super) fn optional_str(entry: &serde_json::Value, field: &str) -> Option<String> {
    entry
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// diff_list_files 响应：session_id 在无会话（SessionNotFound 空响应）时缺失。
pub(super) fn parse_diff_files(
    response: &AppResponseEnvelope,
) -> Result<(Option<String>, Vec<DiffFileSummary>, Option<GitDiffInfo>), String> {
    match &response.response {
        AppResponse::Data(data) => {
            let session_id = optional_str(data, "session_id");
            let files_value = data
                .get("files")
                .ok_or_else(|| "diff list missing files".to_string())?;
            let files = files_value
                .as_array()
                .ok_or_else(|| "diff files is not an array".to_string())?
                .iter()
                .map(|entry| {
                    Ok(DiffFileSummary {
                        path: required_str(entry, "path")?,
                        status: optional_str(entry, "status").unwrap_or_else(|| "unknown".into()),
                        additions: entry
                            .get("additions")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0),
                        deletions: entry
                            .get("deletions")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0),
                        binary: entry
                            .get("binary")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let git = data.get("git").map(|git| GitDiffInfo {
                branch: optional_str(git, "branch"),
                work_dir: optional_str(git, "work_dir"),
                dirty_files: git.get("dirty_files").and_then(serde_json::Value::as_u64),
            });
            Ok((session_id, files, git))
        }
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// diff_get 响应：带回 Host 实际解析的 latest session；files 为空（路径
/// 不在 diff / 无会话）时 file 为 None。
pub(super) fn parse_diff_file(
    response: &AppResponseEnvelope,
) -> Result<(Option<String>, Option<DiffFileDetail>), String> {
    match &response.response {
        AppResponse::Data(data) => {
            let session_id = optional_str(data, "session_id");
            let Some(files) = data.get("files").and_then(serde_json::Value::as_array) else {
                return Err("diff response missing files".into());
            };
            let Some(entry) = files.first() else {
                return Ok((session_id, None));
            };
            let hunks = entry
                .get("hunks")
                .and_then(serde_json::Value::as_array)
                .map(|hunks| {
                    hunks
                        .iter()
                        .map(|hunk| DiffHunkDetail {
                            header: hunk
                                .get("header")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            lines: hunk
                                .get("lines")
                                .and_then(serde_json::Value::as_array)
                                .map(|lines| {
                                    lines
                                        .iter()
                                        .map(|line| DiffLineDetail {
                                            kind: match line
                                                .get("kind")
                                                .and_then(serde_json::Value::as_str)
                                            {
                                                Some("addition") => DiffLineKind::Addition,
                                                Some("deletion") => DiffLineKind::Deletion,
                                                _ => DiffLineKind::Context,
                                            },
                                            text: line
                                                .get("text")
                                                .and_then(serde_json::Value::as_str)
                                                .unwrap_or("")
                                                .to_string(),
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok((
                session_id,
                Some(DiffFileDetail {
                    path: required_str(entry, "path")?,
                    previous_path: entry
                        .get("previous_path")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    status: optional_str(entry, "status").unwrap_or_else(|| "unknown".into()),
                    binary: entry
                        .get("binary")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                    additions: entry
                        .get("additions")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    deletions: entry
                        .get("deletions")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    hunks,
                }),
            ))
        }
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// mcp_list 响应（形状由主代理钉死）：{"servers":[{name,transport,state,tools,last_error}]}。
pub(super) fn parse_mcp_servers(
    response: &AppResponseEnvelope,
) -> Result<Vec<McpServerEntry>, String> {
    match &response.response {
        AppResponse::Data(data) => data
            .get("servers")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "mcp list missing servers".to_string())?
            .iter()
            .map(|entry| {
                Ok(McpServerEntry {
                    name: required_str(entry, "name")?,
                    transport: optional_str(entry, "transport").unwrap_or_else(|| "unknown".into()),
                    state: optional_str(entry, "state").unwrap_or_else(|| "unknown".into()),
                    tool_count: entry
                        .get("tools")
                        .and_then(serde_json::Value::as_array)
                        .map(Vec::len)
                        .unwrap_or(0) as u64,
                    last_error: optional_str(entry, "last_error"),
                })
            })
            .collect(),
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 mcp_test / mcp_server_remove 回执（SET-6c；Data 形状同 mcp_list）；
/// Error 取 Host 脱敏 message 原文（UI 保留旧清单，fail-closed）。
pub(super) fn parse_mcp_receipt(
    response: &AppResponseEnvelope,
) -> Result<Vec<McpServerEntry>, String> {
    match &response.response {
        AppResponse::Data(_) => parse_mcp_servers(response),
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}
