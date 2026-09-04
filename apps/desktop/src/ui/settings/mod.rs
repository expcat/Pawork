//! Settings 壳：导航与共享类型；各页实现见子模块。
//!
//! Settings 壳（SET-3/4/6a/6b/6c/6d/6e/6f/6g）：Settings Rail、「Models &
//! providers」、「General」、「权限与审批」、「工具与 MCP」、「终端」、「外观」
//!、「高级」与「关于」页。
//!
//! 供应商页只呈现 Host `provider_auth_status` 权威事实：供应商名称、
//! 认证方式、连接状态与目录来源（SET-3）；SET-4 增认证写操作（API key
//! secure 输入验证、OAuth 等待/取消、Replace/Remove），全部由 descriptor
//! （auth_methods + auth.type）驱动，禁止按 provider 名分支。SET-6a 增
//! 「General」页（`proxy_url` 读/设置/清除）；SET-6b 增「权限与审批」页
//! （五档审批模式 / 会话信任 / Global 默认只读）；SET-6c 增「工具与
//! MCP」页（复用 Resources 的 mcp_list 数据链 + mcp_test /
//! mcp_server_remove 写动作）；SET-6d 增「终端」页（terminal_settings
//! 读取 + set_terminal_settings 全态写）；SET-6e 复用 Desktop 已有的
//! 100% / 125% / 150% 会话级字号能力，不经 Host；SET-6f 只读展示当前
//! 连接的握手摘要、启动 endpoint、恢复游标，并复用既有 Reconnect。Host 查询失败 /
//! 未知则隐藏对应导航项且不渲染写入口。断线保留 stale 只读结果
//! 并禁用 Host 写动作；外观 / 高级页作为本地能力始终可用。SET-6g 仅在
//! 当前认证握手声明非空 Host 数据目录时显示「关于」，且断线时立即隐藏。
//! 可见 / 键盘 / AX 三路径同 gate。

pub(super) use gpui::{div, prelude::*, px, App, Context, FontWeight, Pixels};

pub(super) use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
pub(super) use crate::ui::components::label::Label;
pub(super) use crate::ui::components::list_row::ListRow;
pub(super) use crate::ui::components::panel::Panel;
pub(super) use crate::ui::theme::{dark, font, metrics};

pub(super) use crate::controller::McpServerEntry;
pub(super) use crate::projection::{
    group_models_by_provider, ApprovalModeWire, AuthStartData, ConnectionState, ModelEntry,
    ProviderAuthState, ProviderAuthStatusEntry, ProviderStatusLabels, SettingsPermissionsState,
    SettingsTerminalState,
};
pub(super) use crate::ui::text_input::TextInput;

pub(super) use super::accessibility::dynamic_identifier;
pub(super) use super::resources::{
    mcp_server_meta_text, mcp_server_name_row, ResourcesFetch, ResourcesPanelState,
};
pub(super) use super::shell_layout;
pub(super) use super::{AppRoute, AppView, SettingsPage};

/// Settings 内容可读列；与 P2 设计的 760–880px 目标一致。
pub(super) const SETTINGS_CONTENT_MAX_WIDTH: f32 = 820.0;
/// Provider 卡片内边距（8px 节奏）。
pub(super) const PROVIDER_CARD_PAD: f32 = 8.0;
/// Provider 普通概览行高度；详情仅在连接流程、错误或二次确认时展开。
pub(crate) const PROVIDER_OVERVIEW_HEIGHT: f32 = 64.0;
/// 写动作按钮高度（与 Composer 28px 动作槽同节奏）。
pub(super) const SETTINGS_ACTION_HEIGHT: f32 = 28.0;
/// 「模型与默认项」区失效提示（render 与 AX 同源；只声明事实，不切换）。
pub(crate) const SETTINGS_DEFAULT_UNAVAILABLE_NOTE: &str = "Default model unavailable — the default provider is disconnected or the model is not in its current catalog.";
/// null `proxy_url` 展示（ADR-047 D1；render / AX 同源）。
pub(crate) const SETTINGS_PROXY_UNSET: &str = "Not set (uses system environment variables)";
/// 生效边界（ADR-047 D2；不得宣称全局即时生效）。
pub(crate) const SETTINGS_PROXY_EFFECT_NOTE: &str = "New OAuth, verification, and catalog requests use this proxy immediately. Model traffic for the active provider updates after switching providers or restarting the Host.";

/// null `trust_workspaces_global` 展示（ADR-048 D1；render / AX 同源）。
pub(crate) const SETTINGS_TRUST_UNSET: &str = "Not set (workspaces are untrusted by default)";
/// 权限页生效边界（ADR-048 D2/D3；不得宣称持久化或影响进行中 Run）。
pub(crate) const SETTINGS_PERMISSIONS_EFFECT_NOTE: &str = "Changes apply only to this session and are not persisted. Running tasks are unchanged; new tasks use the updated settings until the Host restarts.";

/// 「工具与 MCP」页 Remove 二次确认提示（SET-6c / ADR-049 D2；render 与
/// AX 同源，诚实标注快照语义）。
pub(crate) const SETTINGS_MCP_REMOVE_CONFIRM_NOTE: &str = "Removing this server updates the global configuration and clears its credentials. Tools already snapshotted by a running task are unchanged.";
/// 「工具与 MCP」页生效边界（SET-6c / ADR-049 D1/D2；render 与 AX 同源）。
pub(crate) const SETTINGS_MCP_EFFECT_NOTE: &str = "Test checks the server and refreshes its status. Remove updates the global configuration, clears credentials, and unregisters its tools for this session.";

/// null shell 展示（SET-6d / ADR-050 D2；render 与 AX 同源）。
pub(crate) const SETTINGS_TERMINAL_SHELL_UNSET: &str = "Not set (uses the platform default)";
/// 终端页生效边界（SET-6d / ADR-050 D4；render 与 AX 同源，快照语义）。
pub(crate) const SETTINGS_TERMINAL_EFFECT_NOTE: &str =
    "Changes apply to newly created terminals; existing terminals are unchanged.";

/// 外观页主题说明（SET-6e）：只陈述已实现能力，不画未实现的
/// light / system 主题控件。render / AX 同源。
pub(crate) const SETTINGS_APPEARANCE_THEME_NOTE: &str =
    "Dark theme is currently the only theme. macOS Increase Contrast is applied automatically.";
/// 外观页字号生效边界（SET-6e）：本片不引入第二套配置或假持久化。
pub(crate) const SETTINGS_APPEARANCE_EFFECT_NOTE: &str = "Text size applies immediately to this Desktop session and resets to 100% after restart. You can also use Cmd+=, Cmd+-, or Cmd+0.";

/// 高级页启动目标边界（SET-6f）：runtime ID 不能冒充 CLI 配置实例名，
/// 且任何凭证及其路径都不进入 render / AX。
pub(crate) const SETTINGS_ADVANCED_TARGET_NOTE: &str = "The endpoint is selected by --instance or --socket when Desktop starts; changing it requires a restart. The Host runtime ID is not a configuration instance name. GUI tokens and token paths are never shown here.";
/// Host 级自检仍由 pre-Core CLI 命令负责；Desktop 不 shell-out，也不从
/// socket 路径推断 data directory / 配置实例名。
pub(crate) const SETTINGS_ADVANCED_DOCTOR_NOTE: &str = "Use pawork --instance <name> doctor for Host data directory, PID, socket, and handshake checks. Desktop does not infer an instance name or run that command.";

/// Provider 概览中的目录列：只给 availability 与真实模型数，不把错误、
/// endpoint 或 snapshot 标签塞进普通列表与 AX summary。
pub(crate) fn provider_catalog_overview_label(
    provider: &ProviderAuthStatusEntry,
    model_count: usize,
) -> String {
    match &provider.catalog {
        crate::projection::ProviderCatalogState::Remote { .. }
        | crate::projection::ProviderCatalogState::FixedFallback { .. } => {
            if model_count == 0 {
                "Catalog available".into()
            } else {
                format!(
                    "{model_count} model{}",
                    if model_count == 1 { "" } else { "s" }
                )
            }
        }
        crate::projection::ProviderCatalogState::Unavailable { .. } => "Catalog unavailable".into(),
    }
}
/// 外观页字号按钮的固定几何；render 与 AX bounds 共用，避免缩放后命中框漂移。
pub(crate) const SETTINGS_APPEARANCE_CONTROL_HEIGHT: f32 = SETTINGS_ACTION_HEIGHT;
pub(crate) const SETTINGS_APPEARANCE_CONTROL_WIDTH: f32 = 112.0;
pub(crate) const SETTINGS_APPEARANCE_CONTROL_GAP: f32 = 8.0;

/// 外观页唯一可写能力：复用既有三档 `TextScale`。
pub(crate) const SETTINGS_TEXT_SCALES: [font::TextScale; 3] = [
    font::TextScale::Percent100,
    font::TextScale::Percent125,
    font::TextScale::Percent150,
];

/// 字号控件 identifier（render 按钮 / AX 节点 / AX 派发同源）。
pub(crate) const fn settings_text_scale_identifier(scale: font::TextScale) -> &'static str {
    match scale {
        font::TextScale::Percent100 => "settings-text-scale-100",
        font::TextScale::Percent125 => "settings-text-scale-125",
        font::TextScale::Percent150 => "settings-text-scale-150",
    }
}

/// 只接受三个冻结 identifier；未知值 fail-closed。
pub(crate) fn settings_text_scale_from_identifier(identifier: &str) -> Option<font::TextScale> {
    SETTINGS_TEXT_SCALES
        .into_iter()
        .find(|scale| settings_text_scale_identifier(*scale) == identifier)
}

/// 终端页尺寸输入解析（SET-6d）：u16 且 ∈ 2..=1000（与 Host 校验一致，
/// ADR-050 D3）；畸形 / 越界返回 None（Save 禁用，fail-closed）。
/// render 与 AX 同源。
pub(crate) fn parse_terminal_dimension(text: &str) -> Option<u16> {
    let value: u16 = text.trim().parse().ok()?;
    (2..=1000).contains(&value).then_some(value)
}

/// 终端页 shell 输入 → wire 载荷（SET-6d / ADR-050 D3）：trim 后空串
/// 映射为 None（null = 跟随平台默认），使尺寸可在未设置 shell 时
/// 单独保存；非空则回传 trimmed。render / 键盘 / AX 同源。
pub(crate) fn parse_terminal_shell(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 终端页 Save 是否可点（SET-6d）：写 gate 开且 columns/rows 合法。
/// 空 shell 是合法全态（null），不参与禁用。render 与 AX 同源。
pub(crate) fn terminal_save_enabled(writes: bool, columns: Option<u16>, rows: Option<u16>) -> bool {
    writes && columns.is_some() && rows.is_some()
}

/// Settings 供应商页状态行（render 与 AX 同源）。stale / loading / error /
/// 空态独立判定：stale 与 error 可同时出现，空态仅在完全无状态且列表为
/// 空时给出（SET-3 审查修复 2/3）。
pub(crate) fn provider_status_lines(
    state: &crate::projection::SettingsProvidersState,
) -> Vec<(&'static str, String)> {
    let mut lines = Vec::new();
    if let Some(reason) = &state.query.stale_reason {
        lines.push((
            "stale",
            format!("Offline · showing last known state ({reason})"),
        ));
    } else if state.query.loading {
        lines.push(("loading", "Loading…".to_string()));
    }
    if let Some(error) = &state.query.error {
        lines.push(("error", format!("Could not load provider status · {error}")));
    }
    if state.providers.is_empty()
        && !state.query.loading
        && state.query.error.is_none()
        && state.query.stale_reason.is_none()
    {
        lines.push(("empty", "No providers reported by the host.".to_string()));
    }
    lines
}

/// Settings 通用页状态行（render 与 AX 同源）。error 文案由事件消费侧
/// 按动作区分（load vs save），此处原样展示。
pub(crate) fn general_status_lines(
    state: &crate::projection::SettingsGeneralState,
) -> Vec<(&'static str, String)> {
    let mut lines = Vec::new();
    if let Some(reason) = &state.query.stale_reason {
        lines.push((
            "stale",
            format!("Offline · showing last known state ({reason})"),
        ));
    } else if state.query.loading {
        lines.push(("loading", "Loading…".to_string()));
    }
    if let Some(error) = &state.query.error {
        lines.push(("error", error.clone()));
    }
    lines
}

/// Settings 权限页状态行（render 与 AX 同源）。error 文案由事件消费侧
/// 按动作区分（load / set mode / set trust），此处原样展示。
pub(crate) fn permissions_status_lines(
    state: &SettingsPermissionsState,
) -> Vec<(&'static str, String)> {
    let mut lines = Vec::new();
    if let Some(reason) = &state.query.stale_reason {
        lines.push((
            "stale",
            format!("Offline · showing last known state ({reason})"),
        ));
    } else if state.query.loading {
        lines.push(("loading", "Loading…".to_string()));
    }
    if let Some(error) = &state.query.error {
        lines.push(("error", error.clone()));
    }
    lines
}

/// Settings 终端页状态行（SET-6d；render 与 AX 同源）。error 文案由事件
/// 消费侧按动作区分（load / set），此处原样展示。
pub(crate) fn terminal_status_lines(state: &SettingsTerminalState) -> Vec<(&'static str, String)> {
    let mut lines = Vec::new();
    if let Some(reason) = &state.query.stale_reason {
        lines.push((
            "stale",
            format!("Offline · showing last known state ({reason})"),
        ));
    } else if state.query.loading {
        lines.push(("loading", "Loading…".to_string()));
    }
    if let Some(error) = &state.query.error {
        lines.push(("error", error.clone()));
    }
    lines
}

/// Settings「工具与 MCP」页状态行（SET-6c；render 与 AX 同源）。复用
/// Resources 面状态：stale / loading / error / 空态独立判定。
pub(crate) fn tools_status_lines(state: &ResourcesPanelState) -> Vec<(&'static str, String)> {
    let mut lines = Vec::new();
    if let Some(reason) = &state.stale_reason {
        lines.push((
            "stale",
            format!("Offline · showing last known state ({reason})"),
        ));
    } else if matches!(state.fetch, ResourcesFetch::Fetching) {
        lines.push(("loading", "Loading…".to_string()));
    }
    if let ResourcesFetch::Failed(reason) = &state.fetch {
        lines.push(("error", format!("Could not load MCP servers · {reason}")));
    }
    if let Some(error) = &state.action_error {
        lines.push(("action", error.clone()));
    }
    if state.servers.is_empty()
        && !matches!(state.fetch, ResourcesFetch::Fetching)
        && state.stale_reason.is_none()
        && !matches!(state.fetch, ResourcesFetch::Failed(_))
    {
        lines.push(("empty", "No MCP servers configured.".to_string()));
    }
    lines
}

/// Settings 写动作（SET-4）。render / 键盘 / AX 三路径同源：可见按钮、
/// on_activate 与 AX Press 共用同一 identifier 与同一入口 gate。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsAuthAction {
    ConnectApiKey,
    ConnectOauth,
    ReplaceOauth,
    CancelOauth,
    ReplaceApiKey,
    VerifyApiKey,
    CancelApiKeyInput,
    Remove,
    ConfirmRemove,
    KeepRemove,
}

/// Settings 控件 identifier 前缀（action key 在 provider id 之前，前缀
/// 锚定解析无歧义）。
pub(crate) const SETTINGS_CONTROL_PREFIX: &str = "settings-action-";

impl SettingsAuthAction {
    /// 全部动作：key 解析与焦点回收白名单的单一来源。
    pub(crate) const ALL: [Self; 10] = [
        Self::ConnectApiKey,
        Self::ConnectOauth,
        Self::ReplaceOauth,
        Self::CancelOauth,
        Self::ReplaceApiKey,
        Self::VerifyApiKey,
        Self::CancelApiKeyInput,
        Self::Remove,
        Self::ConfirmRemove,
        Self::KeepRemove,
    ];

    pub(crate) fn key(&self) -> &'static str {
        match self {
            Self::ConnectApiKey => "connect-api-key",
            Self::ConnectOauth => "connect-oauth",
            Self::ReplaceOauth => "replace-oauth",
            Self::CancelOauth => "cancel-oauth",
            Self::ReplaceApiKey => "replace-api-key",
            Self::VerifyApiKey => "verify-api-key",
            Self::CancelApiKeyInput => "cancel-api-key",
            Self::Remove => "remove",
            Self::ConfirmRemove => "confirm-remove",
            Self::KeepRemove => "keep-remove",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|action| action.key() == key)
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::ConnectApiKey => "Connect API key",
            Self::ConnectOauth => "Connect OAuth",
            Self::ReplaceOauth => "Replace OAuth",
            Self::CancelOauth => "Cancel",
            Self::ReplaceApiKey => "Replace API key",
            Self::VerifyApiKey => "Verify",
            Self::CancelApiKeyInput => "Cancel",
            Self::Remove => "Remove",
            Self::ConfirmRemove => "Remove connection",
            Self::KeepRemove => "Keep",
        }
    }

    /// 控件 identifier（render 按钮 id / AX 节点 id / 派发键三用；provider
    /// id 经 dynamic_identifier 转义）。
    pub(crate) fn identifier(&self, provider_id: &str) -> String {
        format!(
            "{SETTINGS_CONTROL_PREFIX}{}",
            dynamic_identifier(self.key(), provider_id)
        )
    }
}

/// settings 页控件（AX 派发用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SettingsControl {
    Action(SettingsAuthAction, String),
    ApiKeyInput(String),
    /// 设为默认（SET-5）：携带转义后的 "<provider>:<model>"，由 AppView
    /// 对照 projection.models 还原（未知 fail-closed）。
    SetDefaultModel(String),
}

pub(crate) fn settings_api_key_input_identifier(provider_id: &str) -> String {
    format!(
        "{SETTINGS_CONTROL_PREFIX}{}",
        dynamic_identifier("api-key-input", provider_id)
    )
}

/// 「设为默认」控件 identifier（render 按钮 id / AX 节点 id / 派发键三用；
/// provider 与 model 以 ':' 拼接后整体转义）。
pub(crate) fn settings_set_default_identifier(provider_id: &str, model_id: &str) -> String {
    format!(
        "{SETTINGS_CONTROL_PREFIX}{}",
        dynamic_identifier("set-default", &format!("{provider_id}:{model_id}"))
    )
}

/// 前缀锚定解析 settings 控件 identifier；provider 部分是转义后的 id，
/// 由 AppView 对照 provider 列表还原（未知 id fail-closed）。
pub(crate) fn parse_settings_control(identifier: &str) -> Option<SettingsControl> {
    let rest = identifier.strip_prefix(SETTINGS_CONTROL_PREFIX)?;
    if let Some(provider) = rest.strip_prefix("api-key-input-") {
        return Some(SettingsControl::ApiKeyInput(provider.to_string()));
    }
    if let Some(target) = rest.strip_prefix("set-default-") {
        return Some(SettingsControl::SetDefaultModel(target.to_string()));
    }
    // 已知 action key 集合有限且互不为前缀（均以 '-' 收尾成段），
    // 逐个前缀匹配消解复合 key（connect-oauth 等）。
    for key in [
        "connect-api-key",
        "connect-oauth",
        "replace-oauth",
        "cancel-oauth",
        "replace-api-key",
        "verify-api-key",
        "cancel-api-key",
        "confirm-remove",
        "keep-remove",
        "remove",
    ] {
        if let Some(provider) = rest.strip_prefix(&format!("{key}-")) {
            return Some(SettingsControl::Action(
                SettingsAuthAction::from_key(key)?,
                provider.to_string(),
            ));
        }
    }
    None
}

/// 「工具与 MCP」页写动作（SET-6c / ADR-049）。render / 键盘 / AX 三路径
/// 同源：可见按钮、on_activate 与 AX Press 共用同一 identifier 与同一
/// 入口 gate；Remove 走两步确认（先 Remove 再 ConfirmRemove）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsMcpAction {
    Test,
    Remove,
    ConfirmRemove,
    KeepRemove,
}

/// MCP 控件 identifier 前缀（action key 在 server 名之前，前缀锚定解析
/// 无歧义；与 provider 动作的 SETTINGS_CONTROL_PREFIX 区分）。
pub(crate) const SETTINGS_MCP_CONTROL_PREFIX: &str = "settings-mcp-";

impl SettingsMcpAction {
    pub(crate) fn key(&self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Remove => "remove",
            Self::ConfirmRemove => "confirm-remove",
            Self::KeepRemove => "keep-remove",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        match key {
            "test" => Some(Self::Test),
            "remove" => Some(Self::Remove),
            "confirm-remove" => Some(Self::ConfirmRemove),
            "keep-remove" => Some(Self::KeepRemove),
            _ => None,
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Test => "Test",
            Self::Remove => "Remove",
            Self::ConfirmRemove => "Confirm remove",
            Self::KeepRemove => "Keep",
        }
    }

    /// 控件 identifier（render 按钮 id / AX 节点 id / 派发键三用；server
    /// 名经 dynamic_identifier 转义）。
    pub(crate) fn identifier(&self, server: &str) -> String {
        format!(
            "{SETTINGS_MCP_CONTROL_PREFIX}{}",
            dynamic_identifier(self.key(), server)
        )
    }
}

/// 前缀锚定解析「工具与 MCP」页控件 identifier；server 部分是转义后的
/// 名，由 AppView 对照当前权威清单还原（未知 fail-closed）。
pub(crate) fn parse_settings_mcp_control(identifier: &str) -> Option<(SettingsMcpAction, String)> {
    let rest = identifier.strip_prefix(SETTINGS_MCP_CONTROL_PREFIX)?;
    // key 集合有限；confirm-remove / keep-remove 须先于 remove 匹配。
    for key in ["confirm-remove", "keep-remove", "remove", "test"] {
        if let Some(escaped) = rest.strip_prefix(&format!("{key}-")) {
            return Some((SettingsMcpAction::from_key(key)?, escaped.to_string()));
        }
    }
    None
}

/// 按 Host descriptor（auth_methods + auth.type）推导卡片可见写动作；
/// 未知 method 不臆造入口（fail-closed）。
pub(crate) fn settings_auth_actions(
    provider: &ProviderAuthStatusEntry,
    api_key_editor_open: bool,
    remove_confirm: bool,
    oauth_waiting: bool,
) -> Vec<SettingsAuthAction> {
    let mut actions = Vec::new();
    match provider.auth {
        ProviderAuthState::None | ProviderAuthState::Error { .. } => {
            for method in &provider.auth_methods {
                match method.as_str() {
                    "api_key" => {
                        if api_key_editor_open {
                            actions.push(SettingsAuthAction::VerifyApiKey);
                            actions.push(SettingsAuthAction::CancelApiKeyInput);
                        } else {
                            actions.push(SettingsAuthAction::ConnectApiKey);
                        }
                    }
                    "oauth" => actions.push(SettingsAuthAction::ConnectOauth),
                    _ => {}
                }
            }
        }
        ProviderAuthState::Connecting => {
            // oauth 等待中可取消；api_key 验证是 Host 单次同步请求，
            // 无中途取消（Host auth_cancel 对 api_key 显式拒绝）。
            if oauth_waiting {
                actions.push(SettingsAuthAction::CancelOauth);
            }
        }
        ProviderAuthState::Connected { .. } => {
            for method in &provider.auth_methods {
                match method.as_str() {
                    "api_key" => {
                        if api_key_editor_open {
                            actions.push(SettingsAuthAction::VerifyApiKey);
                            actions.push(SettingsAuthAction::CancelApiKeyInput);
                        } else {
                            actions.push(SettingsAuthAction::ReplaceApiKey);
                        }
                    }
                    "oauth" => actions.push(SettingsAuthAction::ReplaceOauth),
                    _ => {}
                }
            }
            if remove_confirm {
                actions.push(SettingsAuthAction::ConfirmRemove);
                actions.push(SettingsAuthAction::KeepRemove);
            } else {
                actions.push(SettingsAuthAction::Remove);
            }
        }
    }
    actions
}

pub(super) fn status_line(text: &str, color: gpui::Rgba) -> impl IntoElement {
    div().child(
        Label::new(text.to_string())
            .size(font::BODY_SM)
            .color(color),
    )
}

#[cfg(test)]
mod tests {
    use super::{parse_terminal_dimension, parse_terminal_shell, terminal_save_enabled};

    #[test]
    fn terminal_save_allows_empty_shell_when_size_is_valid() {
        assert_eq!(parse_terminal_shell("   "), None);
        assert_eq!(parse_terminal_shell("/bin/zsh"), Some("/bin/zsh".into()));
        assert!(terminal_save_enabled(true, Some(80), Some(24)));
        assert!(!terminal_save_enabled(true, None, Some(24)));
        assert!(!terminal_save_enabled(false, Some(80), Some(24)));
        assert_eq!(parse_terminal_dimension(" 120 "), Some(120));
        assert_eq!(parse_terminal_dimension("1"), None);
        assert_eq!(parse_terminal_dimension("2000"), None);
    }
}

mod about;
mod advanced;
mod appearance;
mod approval_labels;
mod general;
mod permissions;
mod providers;
mod terminal;
mod tools;

pub(crate) use approval_labels::{
    description as approval_mode_description, label as approval_mode_label,
    ALL as APPROVAL_MODE_ALL,
};

impl AppView {
    /// Settings 左栏（SET-3）：返回工作台 + 首个导航项「Models & providers」。
    /// 宽度沿用 TaskRail 的响应式 rail（288 / 240 / 320），进入时整体替换
    /// TaskRail；未接通页面不显示（无假导航项）。
    pub(super) fn settings_rail_element(
        &mut self,
        rail_width: Pixels,
        cx: &mut Context<Self>,
    ) -> Panel {
        let back_focus = self.settings_back_focus.clone();
        let back = Button::new("settings-back")
            .track_focus(&back_focus)
            .variant(ButtonVariant::Raised)
            .padding(ButtonPadding::Horizontal(metrics::RAIL_INNER_PAD))
            .height(px(metrics::RAIL_TOP_ROW_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY)
            .label("← Back to workspace")
            .tooltip("Back to workspace")
            .on_click(cx.listener(|view, event, window, cx| {
                if view.consume_button_key_click("settings-back", event) {
                    return;
                }
                view.on_close_settings(window, cx);
            }))
            .on_activate(cx.listener(|view, _event, window, cx| {
                view.note_button_key_activate("settings-back");
                view.on_close_settings(window, cx);
                cx.stop_propagation();
            }));
        let general_available = self.projection.settings_general.query.available;
        let permissions_available = self.projection.settings_permissions.query.available;
        let tools_available = self.resources.available;
        let terminal_available = self.projection.settings_terminal.query.available;
        let about_available = self.settings_about_rows().is_some();
        let current_page = match self.settings_page {
            SettingsPage::General if !general_available => SettingsPage::Providers,
            SettingsPage::Permissions if !permissions_available => SettingsPage::Providers,
            SettingsPage::Tools if !tools_available => SettingsPage::Providers,
            SettingsPage::Terminal if !terminal_available => SettingsPage::Providers,
            SettingsPage::About if !about_available => SettingsPage::Advanced,
            page => page,
        };
        let mut rail = Panel::side_right(rail_width)
            .child(shell_layout::rail_safe_area())
            .child(
                div()
                    .id("settings-rail-title")
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .px(px(metrics::RAIL_INNER_PAD))
                    .font_weight(FontWeight::MEDIUM)
                    .child(
                        Label::new("Settings")
                            .size(font::TITLE)
                            .color(dark().text.primary),
                    ),
            )
            .child(back)
            .child(self.settings_nav_item(
                "settings-nav-providers",
                "Models & providers",
                current_page == SettingsPage::Providers,
                SettingsPage::Providers,
                cx,
            ));
        if general_available {
            rail = rail.child(self.settings_nav_item(
                "settings-nav-general",
                "General",
                current_page == SettingsPage::General,
                SettingsPage::General,
                cx,
            ));
        }
        if permissions_available {
            rail = rail.child(self.settings_nav_item(
                "settings-nav-permissions",
                "Approvals",
                current_page == SettingsPage::Permissions,
                SettingsPage::Permissions,
                cx,
            ));
        }
        if tools_available {
            rail = rail.child(self.settings_nav_item(
                "settings-nav-tools",
                "Tools & MCP",
                current_page == SettingsPage::Tools,
                SettingsPage::Tools,
                cx,
            ));
        }
        if terminal_available {
            rail = rail.child(self.settings_nav_item(
                "settings-nav-terminal",
                "Terminal",
                current_page == SettingsPage::Terminal,
                SettingsPage::Terminal,
                cx,
            ));
        }
        rail = rail.child(self.settings_nav_item(
            "settings-nav-appearance",
            "Appearance",
            current_page == SettingsPage::Appearance,
            SettingsPage::Appearance,
            cx,
        ));
        rail = rail.child(self.settings_nav_item(
            "settings-nav-advanced",
            "Advanced",
            current_page == SettingsPage::Advanced,
            SettingsPage::Advanced,
            cx,
        ));
        if about_available {
            rail = rail.child(self.settings_nav_item(
                "settings-nav-about",
                "About",
                current_page == SettingsPage::About,
                SettingsPage::About,
                cx,
            ));
        }
        rail
    }

    /// Settings 全宽内容区（SET-4 认证写操作）。状态行全部来自
    /// projection（Host 权威 / stale / error）；卡片动作由 descriptor 驱动，
    /// 断线（stale）时可见 / 键盘 / AX 同时禁用。
    pub(super) fn settings_page_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.settings_page == SettingsPage::General
            && self.projection.settings_general.query.available
        {
            return self.settings_general_page_element(cx).into_any_element();
        }
        if self.settings_page == SettingsPage::Permissions
            && self.projection.settings_permissions.query.available
        {
            return self
                .settings_permissions_page_element(cx)
                .into_any_element();
        }
        if self.settings_page == SettingsPage::Tools && self.resources.available {
            return self.settings_tools_page_element(cx).into_any_element();
        }
        if self.settings_page == SettingsPage::Terminal
            && self.projection.settings_terminal.query.available
        {
            return self.settings_terminal_page_element(cx).into_any_element();
        }
        if self.settings_page == SettingsPage::Appearance {
            return self.settings_appearance_page_element(cx).into_any_element();
        }
        if self.settings_page == SettingsPage::Advanced {
            return self.settings_advanced_page_element(cx).into_any_element();
        }
        if self.settings_page == SettingsPage::About {
            if self.settings_about_rows().is_some() {
                return self.settings_about_page_element().into_any_element();
            }
            return self.settings_advanced_page_element(cx).into_any_element();
        }
        self.settings_providers_page_element(cx).into_any_element()
    }

    fn settings_nav_item(
        &mut self,
        id: &'static str,
        label: &'static str,
        selected: bool,
        page: SettingsPage,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let focus = match page {
            SettingsPage::General => self.settings_nav_general_focus.clone(),
            SettingsPage::Permissions => self.settings_nav_permissions_focus.clone(),
            SettingsPage::Tools => self.settings_nav_tools_focus.clone(),
            SettingsPage::Terminal => self.settings_nav_terminal_focus.clone(),
            SettingsPage::Appearance => self.settings_nav_appearance_focus.clone(),
            SettingsPage::Advanced => self.settings_nav_advanced_focus.clone(),
            SettingsPage::About => self.settings_nav_about_focus.clone(),
            SettingsPage::Providers => self.settings_nav_providers_focus.clone(),
        };
        if selected {
            return div()
                .id(id)
                .tab_stop(true)
                .track_focus(&focus)
                .focus(|style| style.border_1().border_color(dark().accent.primary))
                .mt_2()
                .w_full()
                .h(px(metrics::RAIL_TOP_ROW_HEIGHT))
                .flex()
                .items_center()
                .px(px(metrics::RAIL_INNER_PAD))
                .rounded(px(4.0))
                .bg(dark().surface.raised)
                .child(
                    div().font_weight(FontWeight::MEDIUM).child(
                        Label::new(label)
                            .size(font::BODY_SM)
                            .color(dark().text.primary),
                    ),
                )
                .into_any_element();
        }
        Button::new(id)
            .track_focus(&focus)
            .variant(ButtonVariant::Ghost)
            .padding(ButtonPadding::Horizontal(metrics::RAIL_INNER_PAD))
            .height(px(metrics::RAIL_TOP_ROW_HEIGHT))
            .vcenter()
            .radius(4.0)
            .text_size(font::BODY_SM)
            .label(label)
            .on_click(cx.listener(move |view, event, window, cx| {
                if view.consume_button_key_click(id, event) {
                    return;
                }
                view.on_select_settings_page(page, window, cx);
            }))
            .on_activate(cx.listener(move |view, _event, window, cx| {
                view.note_button_key_activate(id);
                view.on_select_settings_page(page, window, cx);
                cx.stop_propagation();
            }))
            .into_any_element()
    }

    /// 供应商页写操作 gate（SET-4/5）：连接 + 非 stale。页始终可见，
    /// `available` 默认 true，与 `SettingsQueryGate::writes_enabled` 同口径。
    pub(crate) fn settings_writes_enabled(&self) -> bool {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        self.projection.settings_providers.writes_enabled(connected)
    }

    /// 通用页写操作 gate（SET-6a）：连接 + 非 stale + 查询已成功。
    pub(crate) fn settings_general_writes_enabled(&self) -> bool {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        self.projection.settings_general.writes_enabled(connected)
    }

    /// 权限页写操作 gate（SET-6b）：连接 + 非 stale + 查询已成功。
    pub(crate) fn settings_permissions_writes_enabled(&self) -> bool {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        self.projection
            .settings_permissions
            .writes_enabled(connected)
    }

    /// 终端页写操作 gate（SET-6d）：连接 + 非 stale + 查询已成功。
    pub(crate) fn settings_terminal_writes_enabled(&self) -> bool {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        self.projection.settings_terminal.writes_enabled(connected)
    }

    /// 「工具与 MCP」页写操作 gate（SET-6c）：连接 + 非 stale + mcp_list
    /// 已成功（available；语义与通用 / 权限页一致）。
    pub(crate) fn settings_tools_writes_enabled(&self) -> bool {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        connected && self.resources.available && self.resources.stale_reason.is_none()
    }
}
