//! 最小 i18n：English / 中文 界面语言切换（本地 presentation preference）。
//!
//! 与 `TextScale` 同口径：即时生效，保存到用户目录 desktop.json，重启恢复；不引入任何外部 i18n 框架。`render` 与 `AX` 必须经同一个
//! `t()` 取文案（同源合同）；AX node id 保持英文稳定标识，不翻译。
//! 数据内容（会话标题、provider / model id、路径、wire 错误详情）不翻译。

use std::sync::atomic::{AtomicU8, Ordering};

/// 界面语言。默认 English：既有测试与 AX 断言全部基于默认语言。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    #[default]
    English,
    Chinese,
}

impl Language {
    /// 各语言用自身语言书写的显示名（语言切换器惯例）。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Chinese => "中文",
        }
    }

    /// 稳定标识（按钮 / AX action identifier；不翻译）。
    pub const fn identifier(self) -> &'static str {
        match self {
            Self::English => "settings-language-en",
            Self::Chinese => "settings-language-zh",
        }
    }
}

/// 由 AX action identifier 反查语言；未知 id 返回 None（fail-closed）。
/// 与 settings_text_scale_from_identifier 同口径，供 AX press 派发用。
pub fn language_from_identifier(identifier: &str) -> Option<Language> {
    LANGUAGES
        .iter()
        .copied()
        .find(|language| language.identifier() == identifier)
}

/// 可选语言全集（切换器按序渲染）。
pub const LANGUAGES: [Language; 2] = [Language::English, Language::Chinese];

static CURRENT: AtomicU8 = AtomicU8::new(Language::English as u8);

/// 当前界面语言（全局读；渲染与 AX 同源入口）。
pub fn language() -> Language {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Language::Chinese,
        _ => Language::English,
    }
}

/// 切换界面语言（由 AppView 触发并重渲染）。
pub fn set_language(language: Language) {
    CURRENT.store(language as u8, Ordering::Relaxed);
}

/// 按 key 返回本地化文案：每个 arm 同时给出 (English, 中文)；
/// 未知 key 原样返回，使漏译在界面上可见而不是静默空白。
pub fn t(key: &'static str) -> &'static str {
    localize(key, language())
}

/// 双占位符模板：按序替换前两个 `{}`（如 Resume 重放区间）。
pub fn t2(key: &'static str, first: &str, second: &str) -> String {
    localize_t2(key, first, second, language())
}

/// Provider 目录概览文案（0 模型只报 availability；复数按语言处理）。
pub fn catalog_overview_label(model_count: usize) -> String {
    localize_catalog_overview(model_count, language())
}

/// 纯函数：给定语言取 key 文案，不读全局状态（单测无副作用、不与并行测试竞态）。
fn localize(key: &'static str, lang: Language) -> &'static str {
    let (en, zh) = match key {
        // ── Settings · Appearance 语言切换 ──
        "settings.appearance.language" => ("Language", "语言"),
        "settings.appearance.language.current" => ("Current · {}", "当前 · {}"),
        "settings.appearance.language.tooltip_current" => {
            ("Current interface language: {}", "当前界面语言：{}")
        }
        "settings.appearance.language.tooltip_set" => {
            ("Switch interface language to {}", "切换界面语言为 {}")
        }
        "settings.appearance.language.hint" => (
            "Applies immediately and is restored after restart.",
            "立即生效，并在重启后恢复。",
        ),
        // ── Settings · Appearance 页 ──
        "settings.appearance.load_failed" => {
            ("Could not load appearance preferences", "无法读取外观设置")
        }
        "settings.appearance.save_failed" => {
            ("Could not save appearance preferences", "无法保存外观设置")
        }
        "settings.appearance.title" => ("Appearance", "外观"),
        "settings.appearance.subtitle" => {
            ("Desktop presentation preferences", "桌面显示偏好")
        }
        "settings.appearance.theme" => ("Theme · Dark", "主题 · 深色"),
        "settings.appearance.theme_note" => (
            "Dark theme is currently the only theme.",
            "当前仅提供深色主题。",
        ),
        "settings.appearance.text_size" => ("Text size", "字号"),
        "settings.appearance.current_scale" => ("Current · {}%", "当前 · {}%"),
        "settings.appearance.tooltip_scale_current" => {
            ("Current text size: {}%", "当前字号：{}%")
        }
        "settings.appearance.tooltip_scale_set" => ("Set text size to {}%", "将字号设为 {}%"),
        "settings.appearance.effect_note" => (
            "Text size is saved and restored after restart. You can also use Cmd+=, Cmd+-, or Cmd+0.",
            "字号立即生效，保存后重启仍在。也可使用 Cmd+=、Cmd+- 或 Cmd+0。",
        ),
        "settings.appearance.sample_body" => (
            "The quick brown fox jumps over the lazy dog.",
            "敏捷的棕色狐狸跳过了懒狗。",
        ),
        "settings.appearance.sample_sub" => (
            "Code, tools, and review stay readable at this size.",
            "代码、工具与审阅在此字号下保持清晰可读。",
        ),
        "settings.appearance.scale_button" => ("Text size {}%", "字号 {}%"),
        "settings.appearance.state_current" => ("Current", "当前"),
        "settings.appearance.state_available" => ("Available", "可选"),
        "settings.appearance.sample_title" => ("Text size preview", "字号预览"),
        "settings.appearance.scope_title" => ("Scope", "生效范围"),
        // ── Settings 壳 · 导航 / 通用动作 ──
        "settings.rail_title" => ("Settings", "设置"),
        "settings.back" => ("← Back to workspace", "← 返回工作台"),
        "settings.back_tooltip" => ("Back to workspace", "返回工作台"),
        "settings.nav.providers" => ("Models & providers", "模型与提供商"),
        "settings.nav.general" => ("Network", "网络"),
        "settings.nav.permissions" => ("Approvals", "审批"),
        "settings.nav.tools" => ("Tools & MCP", "工具与 MCP"),
        "settings.nav.terminal" => ("Terminal", "终端"),
        "settings.nav.appearance" => ("Appearance", "外观"),
        "settings.nav.advanced" => ("Advanced", "高级"),
        "settings.nav.about" => ("About", "关于"),
        "settings.nav.state_selected" => ("Selected", "已选中"),
        "settings.refresh" => ("Refresh", "刷新"),
        "settings.save" => ("Save", "保存"),
        "settings.clear" => ("Clear", "清除"),
        "settings.current" => ("Current · {}", "当前 · {}"),
        "settings.status.offline_stale" => {
            ("Offline · showing last known state ({})", "离线 · 显示最后已知状态（{}）")
        }
        "settings.status.loading" => ("Loading…", "加载中…"),
        "settings.default_unavailable_note" => (
            "Default model unavailable — the default provider is disconnected or the model is not in its current catalog.",
            "默认模型不可用——默认提供商已断开连接，或该模型不在其当前目录中。",
        ),
        // ── Settings · Network（General 页）──
        "settings.network.title" => ("Network", "网络"),
        "settings.network.subtitle" => {
            ("Host outbound network settings", "Host 出站网络设置")
        }
        "settings.network.refresh_tooltip" => ("Refresh network settings", "刷新网络设置"),
        "settings.network.save_tooltip" => ("Save proxy URL", "保存代理 URL"),
        "settings.network.clear_tooltip" => ("Clear proxy URL", "清除代理 URL"),
        "settings.network.proxy_title" => ("HTTP proxy", "HTTP 代理"),
        "settings.network.proxy_unset" => (
            "Not set (uses system environment variables)",
            "未设置（使用系统环境变量）",
        ),
        "settings.network.proxy_effect_note" => (
            "New OAuth, verification, and catalog requests use this proxy immediately. Model traffic for the active provider updates after switching providers or restarting the Host.",
            "新的 OAuth、验证与目录请求立即使用此代理。当前提供商的模型流量将在切换提供商或重启 Host 后更新。",
        ),
        "settings.network.proxy_storage_note" => (
            "Stored in Pawork's per-user config.toml outside all workspaces. Proxy values in workspace .pawork/config.toml files are ignored.",
            "存储在所有 workspace 之外的 Pawork 按用户 config.toml 中。workspace .pawork/config.toml 文件中的代理值会被忽略。",
        ),
        "settings.network.ax_status" => ("Network status", "网络状态"),
        "settings.network.ax_current_proxy" => ("Current proxy URL", "当前代理 URL"),
        "settings.network.ax_proxy_input" => ("Proxy URL", "代理 URL"),
        "settings.network.ax_effect" => ("Effect", "生效边界"),
        "settings.network.ax_storage" => ("Storage", "存储位置"),
        // ── Settings · Approvals（Permissions 页）──
        "settings.permissions.title" => ("Approvals", "审批"),
        "settings.permissions.subtitle" => (
            "Saved approval default and current workspace trust",
            "持久化审批默认与当前 workspace 信任",
        ),
        "settings.permissions.refresh_tooltip" => {
            ("Refresh permissions settings", "刷新权限设置")
        }
        "settings.permissions.unknown_mode" => ("Unknown", "未知"),
        "settings.permissions.mode_title" => ("Approval mode", "审批模式"),
        "settings.permissions.trust_remove" => ("Remove trust", "移除信任"),
        "settings.permissions.trust_add" => ("Trust workspace", "信任 workspace"),
        "settings.permissions.trust_toggle_tooltip" => (
            "Save trust for the current workspace",
            "保存当前 workspace 信任",
        ),
        "settings.permissions.trust_state_trusted" => ("Trusted", "已信任"),
        "settings.permissions.trust_state_untrusted" => ("Not trusted", "未信任"),
        "settings.permissions.session_trust_title" => ("Workspace trust", "项目信任"),
        "settings.permissions.session_trust_desc" => (
            "Remember trust for this workspace after restart",
            "记住当前 workspace 信任，重启后恢复",
        ),
        "settings.permissions.global_readonly" => {
            ("Global default (read only) · {}", "全局默认（只读）· {}")
        }
        "settings.permissions.global_trust_all" => {
            ("Set to trust all workspaces", "已设为信任所有 workspace")
        }
        "settings.permissions.global_distrust_all" => {
            ("Set to distrust all workspaces", "已设为不信任所有 workspace")
        }
        "settings.permissions.trust_unset" => (
            "Not set (workspaces are untrusted by default)",
            "未设置（workspace 默认不受信任）",
        ),
        "settings.permissions.effect_note" => (
            "Saved to global configuration. Approval mode is the default; trust applies to this workspace. Running tasks are unchanged. Explicit launch options override saved values for that launch.",
            "保存到全局配置：审批模式作为默认，信任只针对当前项目。进行中的任务不变；显式启动参数覆盖当次进程。",
        ),
        "settings.permissions.state_current" => ("Current", "当前"),
        "settings.permissions.ax_status" => ("Permissions status", "权限状态"),
        "settings.permissions.ax_global_title" => ("Global default", "全局默认"),
        "settings.permissions.ax_effect" => ("Effect", "生效边界"),
        "settings.permissions.ax_current_mode_desc" => {
            ("Current approval mode", "当前审批模式")
        }
        // ── Settings · Tools & MCP ──
        "settings.tools.title" => ("Tools & MCP", "工具与 MCP"),
        "settings.tools.subtitle" => (
            "MCP servers, status, and configuration reported by the Host",
            "Host 报告的 MCP 服务器、状态与配置",
        ),
        "settings.tools.refresh_tooltip" => ("Refresh MCP servers", "刷新 MCP 服务器"),
        "settings.tools.effect_note" => (
            "Test checks the server and refreshes its status. Remove updates the global configuration, clears credentials, and unregisters its tools for this session.",
            "Test 检查服务器并刷新其状态。Remove 更新全局配置、清除凭证，并为本次会话注销其工具。",
        ),
        "settings.tools.remove_confirm_note" => (
            "Removing this server updates the global configuration and clears its credentials. Tools already snapshotted by a running task are unchanged.",
            "移除此服务器将更新全局配置并清除其凭证。已被运行中任务快照的工具不受影响。",
        ),
        "settings.tools.tooltip_test" => {
            ("Ping this server and refresh its state.", "检测此服务器并刷新其状态。")
        }
        "settings.tools.tooltip_remove" => (
            "Remove this server from the Global config and clear its credentials.",
            "从 Global 配置中移除此服务器并清除其凭证。",
        ),
        "settings.tools.status_error" => ("Could not load MCP servers · {}", "无法加载 MCP 服务器 · {}"),
        "settings.tools.status_empty" => ("No MCP servers configured.", "尚未配置 MCP 服务器。"),
        "settings.tools.action_test" => ("Test", "测试"),
        "settings.tools.action_remove" => ("Remove", "移除"),
        "settings.tools.action_confirm_remove" => ("Confirm remove", "确认移除"),
        "settings.tools.action_keep" => ("Keep", "保留"),
        "settings.tools.ax_status" => ("Tools status", "工具状态"),
        "settings.tools.ax_effect" => ("Effect", "生效边界"),
        "settings.tools.ax_server_summary" => ("{} · {} · {} tools", "{} · {} · {} 个工具"),
        // ── Settings · Terminal ──
        "settings.terminal.title" => ("Terminal", "终端"),
        "settings.terminal.subtitle" => (
            "Default shell and size for new terminals",
            "新终端的默认 shell 与尺寸",
        ),
        "settings.terminal.refresh_tooltip" => ("Refresh terminal settings", "刷新终端设置"),
        "settings.terminal.save_tooltip" => (
            "Save terminal defaults (shell, columns, rows)",
            "保存终端默认值（shell、列、行）",
        ),
        "settings.terminal.clear_tooltip" => ("Clear default shell", "清除默认 shell"),
        "settings.terminal.current_shell" => ("Current shell · {}", "当前 shell · {}"),
        "settings.terminal.current_size" => ("Current size · {}", "当前尺寸 · {}"),
        "settings.terminal.shell_label" => ("Shell", "Shell"),
        "settings.terminal.size_label" => ("Size", "尺寸"),
        "settings.terminal.shell_unset" => (
            "Not set (uses the platform default)",
            "未设置（使用平台默认）",
        ),
        "settings.terminal.effect_note" => (
            "Changes apply to newly created terminals; existing terminals are unchanged.",
            "更改仅对新创建的终端生效；已有终端不受影响。",
        ),
        "settings.terminal.ax_status" => ("Terminal status", "终端状态"),
        "settings.terminal.ax_default_shell" => ("Default shell", "默认 shell"),
        "settings.terminal.ax_default_size" => ("Default size", "默认尺寸"),
        "settings.terminal.ax_columns" => ("Columns", "列数"),
        "settings.terminal.ax_rows" => ("Rows", "行数"),
        "settings.terminal.ax_effect" => ("Effect", "生效边界"),
        // ── Settings · Advanced ──
        "settings.advanced.title" => ("Advanced", "高级"),
        "settings.advanced.subtitle" => (
            "Connection diagnostics and startup target",
            "连接诊断与启动目标",
        ),
        "settings.advanced.reconnect" => ("Reconnect", "重新连接"),
        "settings.advanced.connected" => ("Connected", "已连接"),
        "settings.advanced.unavailable_connect" => {
            ("Unavailable · connect to the Host", "不可用 · 请连接 Host")
        }
        "settings.advanced.none_granted" => ("None granted", "未授予"),
        "settings.advanced.fresh_snapshot" => ("Fresh snapshot", "全新快照"),
        "settings.advanced.unavailable" => ("Unavailable", "不可用"),
        "settings.advanced.row_connection" => ("Connection", "连接"),
        "settings.advanced.row_runtime" => ("Host runtime ID", "Host 运行时 ID"),
        "settings.advanced.row_api" => ("GUI API", "GUI API"),
        "settings.advanced.row_capabilities" => ("Granted capabilities", "已授予能力"),
        "settings.advanced.row_endpoint" => ("Endpoint", "端点"),
        "settings.advanced.row_resume" => ("Resume", "恢复"),
        "settings.advanced.row_last_ack" => {
            ("Last acknowledged sequence", "最后确认序列号")
        }
        "settings.advanced.target_note" => (
            "The endpoint is selected by --instance or --socket when Desktop starts; changing it requires a restart. The Host runtime ID is not a configuration instance name. GUI tokens and token paths are never shown here.",
            "端点在 Desktop 启动时由 --instance 或 --socket 选定；更改需要重启。Host 运行时 ID 不是配置实例名。GUI token 及其路径永远不会在此显示。",
        ),
        "settings.advanced.doctor_note" => (
            "Use pawork --instance <name> doctor for Host data directory, PID, socket, and handshake checks. Desktop does not infer an instance name or run that command.",
            "请使用 pawork --instance <name> doctor 检查 Host 数据目录、PID、socket 与握手。Desktop 不会推断实例名，也不会执行该命令。",
        ),
        "settings.advanced.ax_target_title" => ("Startup target boundary", "启动目标边界"),
        "settings.advanced.ax_doctor_title" => ("Host diagnostics boundary", "Host 诊断边界"),
        // ── Settings · About ──
        "settings.about.title" => ("About", "关于"),
        "settings.about.subtitle" => (
            "Build and current Host connection information",
            "构建与当前 Host 连接信息",
        ),
        "settings.about.row_desktop_build" => ("Desktop build", "Desktop 构建"),
        "settings.about.row_api" => ("GUI API", "GUI API"),
        "settings.about.row_data_dir" => ("Host data directory", "Host 数据目录"),
        // ── Settings · Providers / Default model ──
        "settings.providers.title" => ("Models & providers", "模型与提供商"),
        "settings.providers.subtitle" => (
            "Connection status and catalog source for each provider",
            "每个提供商的连接状态与目录来源",
        ),
        "settings.providers.refresh_tooltip" => {
            ("Refresh provider status and model catalog", "刷新提供商状态与模型目录")
        }
        "settings.providers.section_providers" => ("Providers", "提供商"),
        "settings.providers.no_auth_method" => ("No auth method", "无认证方式"),
        "settings.providers.tooltip_remove_credential" => {
            ("Remove the stored credential.", "移除已存储的凭证。")
        }
        "settings.providers.proxy_on" => ("Proxy on", "走代理"),
        "settings.providers.proxy_off" => ("Proxy off", "直连"),
        "settings.providers.proxy_tooltip_on" => (
            "This provider connects through the global proxy. Activate to bypass.",
            "该提供商经全局代理连接。点击切换为直连。",
        ),
        "settings.providers.proxy_tooltip_off" => (
            "This provider bypasses the global proxy. Activate to use it.",
            "该提供商绕过全局代理直连。点击切换为经代理连接。",
        ),
        "settings.providers.ax_use_proxy" => ("Use proxy toggle", "代理开关"),
        "settings.providers.authorize_at" => ("Authorize at {}", "前往授权：{}"),
        "settings.providers.oauth_code" => ("Code {}", "代码 {}"),
        "settings.providers.oauth_expires" => ("Expires {}", "到期时间 {}"),
        "settings.providers.connection_error" => ("Connection error · {}", "连接错误 · {}"),
        "settings.providers.endpoint_row" => ("Endpoint · {}", "端点 · {}"),
        "settings.providers.api_key_empty" => ("API key is empty.", "API key 为空。"),
        "settings.providers.default_model_title" => ("Default model", "默认模型"),
        "settings.providers.default_model_subtitle" => (
            "Choose the model used when a new task starts",
            "选择新任务启动时使用的模型",
        ),
        "settings.providers.no_models" => {
            ("No models reported by the host.", "Host 未报告任何模型。")
        }
        "settings.providers.set_default" => ("Set default", "设为默认"),
        "settings.providers.default_badge" => ("Default", "默认"),
        "settings.providers.api_key_placeholder" => ("Paste API key", "粘贴 API key"),
        "settings.providers.catalog_unavailable" => ("Catalog unavailable", "目录不可用"),
        "settings.providers.catalog_available" => ("Catalog available", "目录可用"),
        "settings.providers.status_error" => {
            ("Could not load provider status · {}", "无法加载提供商状态 · {}")
        }
        "settings.providers.status_empty" => {
            ("No providers reported by the host.", "Host 未报告任何提供商。")
        }
        "settings.providers.action_connect_api_key" => ("Connect API key", "连接 API key"),
        "settings.providers.action_connect_oauth" => ("Connect OAuth", "连接 OAuth"),
        "settings.providers.action_replace_oauth" => ("Replace OAuth", "替换 OAuth"),
        "settings.providers.action_cancel" => ("Cancel", "取消"),
        "settings.providers.action_replace_api_key" => ("Replace API key", "替换 API key"),
        "settings.providers.action_verify" => ("Verify", "验证"),
        "settings.providers.action_remove" => ("Remove", "移除"),
        "settings.providers.action_confirm_remove" => ("Remove connection", "移除连接"),
        "settings.providers.action_keep" => ("Keep", "保留"),
        "settings.providers.ax_status" => ("Provider status", "提供商状态"),
        "settings.providers.ax_connection" => ("Connection", "连接"),
        "settings.providers.ax_catalog" => ("Catalog", "目录"),
        "settings.providers.ax_details" => ("Provider details", "提供商详情"),
        "settings.providers.ax_api_key" => ("API key", "API key"),
        "settings.providers.ax_models_title" => ("Models", "模型"),
        // ── 审批模式（render / AX 同源）──
        "approval.mode.always_ask" => ("Always ask", "总是询问"),
        "approval.mode.ask_for_writes" => ("Ask for writes", "写入时询问"),
        "approval.mode.ask_for_dangerous" => {
            ("Ask for dangerous actions", "危险操作时询问")
        }
        "approval.mode.never_ask" => ("Never ask", "从不询问"),
        "approval.mode.read_only" => ("Read only", "只读"),
        "approval.mode_desc.always_ask" => {
            ("Require approval for every tool call", "每次工具调用都需要审批")
        }
        "approval.mode_desc.ask_for_writes" => {
            ("Allow reads; require approval for writes", "允许读取；写入需要审批")
        }
        "approval.mode_desc.ask_for_dangerous" => (
            "Allow routine actions; ask before dangerous actions",
            "允许常规操作；危险操作前询问",
        ),
        "approval.mode_desc.never_ask" => (
            "Run automatically; the Host still blocks catastrophic commands",
            "自动运行；Host 仍会拦截灾难性命令",
        ),
        "approval.mode_desc.read_only" => (
            "Allow read-only actions and block all writes",
            "允许只读操作并拦截所有写入",
        ),
        // ── 连接 / 恢复 / 时间分组 / 任务状态 ──
        "connection.connecting" => ("Connecting…", "连接中…"),
        "connection.connected" => ("Connected · {}", "已连接 · {}"),
        "connection.disconnected" => ("Disconnected · {}", "已断开 · {}"),
        "connection.failed" => ("Connect failed · {}", "连接失败 · {}"),
        "resume.replay" => ("Replay · {}–{}", "重放 · {}–{}"),
        "resume.snapshot_required" => ("Snapshot required · from {}", "需要快照 · 从 {} 开始"),
        "resume.up_to_date" => ("Up to date · {}", "已是最新 · {}"),
        "date.today" => ("Today", "今天"),
        "date.yesterday" => ("Yesterday", "昨天"),
        "date.previous_7_days" => ("Previous 7 days", "最近 7 天"),
        "date.earlier" => ("Earlier", "更早"),
        "live.running" => ("Running", "运行中"),
        "live.needs_input" => ("Needs input", "等待输入"),
        "live.blocked" => ("Blocked", "受阻"),
        "taskrail.unassigned" => ("Unassigned", "未分组"),
        "taskrail.rename" => ("Rename", "重命名"),
        "taskrail.archive" => ("Archive", "归档"),
        // ── Workspace chrome · Composer / Input area ──
        "composer.placeholder_running" => (
            "Run in progress — sending is disabled. Cancel remains available.",
            "任务运行中——发送已禁用，仍可取消。",
        ),
        "composer.placeholder_message" => (
            "Message Pawork… (Enter to send, Shift+Enter for newline)",
            "给 Pawork 发消息…（Enter 发送，Shift+Enter 换行）",
        ),
        "composer.placeholder_open_session" => {
            ("Open a session to send messages.", "打开会话后才能发送消息。")
        }
        "composer.placeholder_waiting" => ("Waiting for connection…", "等待连接…"),
        "composer.placeholder_disconnected" => (
            "Disconnected — click Reconnect before sending.",
            "已断开连接——请先点击重新连接再发送。",
        ),
        "composer.placeholder_connect_failed" => (
            "Connect failed — click Reconnect.",
            "连接失败——请点击重新连接。",
        ),
        "composer.send_disabled_empty" => ("Message is empty.", "消息为空。"),
        "composer.model_tooltip" => ("Select model · {} / {}", "选择模型 · {} / {}"),
        "composer.model_tooltip_none" => ("Select model", "选择模型"),
        "composer.model_loading" => ("Model · loading", "模型 · 加载中"),
        "composer.model_select" => ("Model · select", "模型 · 请选择"),
        "composer.model_disabled_running" => (
            "Model switch is disabled while a run is in progress.",
            "任务运行中不能切换模型。",
        ),
        "composer.model_disabled_loading" => {
            ("Model catalog is still loading.", "模型目录仍在加载。")
        }
        "composer.model_disabled_offline" => {
            ("Model switch needs a live connection.", "切换模型需要有效连接。")
        }
        "composer.workspace_scope" => ("Workspace · {}", "工作区 · {}"),
        "composer.no_project_chip" => ("No project", "无项目"),
        "composer.file_tools_unavailable" => (
            "File tools unavailable until a project is selected.",
            "选择项目前，文件工具不可用。",
        ),
        "common.add_project" => ("Add project…", "添加项目…"),
        // ── Workspace chrome · Task rail ──
        "rail.reconnect" => ("Reconnect", "重新连接"),
        "rail.local" => ("Local", "本地"),
        "rail.tooltip_settings" => ("Settings", "设置"),
        "rail.no_tasks" => ("No tasks", "暂无任务"),
        "rail.scope_all_projects" => ("All projects", "所有项目"),
        "rail.connection_local_connected_resume" => {
            ("Local · Connected · {}", "本地 · 已连接 · {}")
        }
        "rail.connection_local_connected" => ("Local · Connected", "本地 · 已连接"),
        "rail.newtask_available" => ("Create task is available.", "可以创建任务。"),
        "rail.newtask_needs_connection" => {
            ("New task needs a live connection.", "新建任务需要有效连接。")
        }
        "rail.newtask_disabled_disconnected" => (
            "New task disabled · disconnected · {}",
            "新建任务不可用 · 连接已断开 · {}",
        ),
        "rail.newtask_disabled_failed" => (
            "New task disabled · connect failed · {}",
            "新建任务不可用 · 连接失败 · {}",
        ),
        "rail.grouping_timeline_view" => ("Timeline view", "时间线视图"),
        "rail.grouping_projects_view" => ("Projects view", "项目视图"),
        "rail.grouping_show_projects" => ("Show projects", "显示项目"),
        "rail.grouping_show_timeline" => ("Show timeline", "显示时间线"),
        // ── Workspace chrome · Timeline / 条目动作 ──
        "timeline.new_task" => ("New task", "新建任务"),
        "timeline.new_task_tooltip" => ("New task (Cmd+N)", "新建任务（Cmd+N）"),
        "timeline.back_to_bottom" => ("↓ Back to bottom", "↓ 回到底部"),
        "timeline.ax_back_to_bottom" => ("Back to bottom", "回到底部"),
        "timeline.fork" => ("Fork", "分叉"),
        "timeline.review_changes" => ("Review changes", "查看变更"),
        "timeline.empty_title" => ("Start a task", "开始一个任务"),
        "timeline.empty_hint" => (
            "Choose a task from the sidebar or create a new one.",
            "从侧栏选择一个任务，或新建一个任务。",
        ),
        "tool.status_completed" => ("Completed", "已完成"),
        // ── Workspace chrome · Composer context meter ──
        "composer.context_meter" => ("Context · — / {}", "上下文 · — / {}"),
        "composer.context_unavailable" => ("Context · unavailable", "上下文 · 不可用"),
        // ── Run 摘要 / 页脚终态（render 与 AX 同源；失败原因取 wire 原文不翻译）──
        "run.ready_for_review" => ("Ready for review", "待审阅"),
        "run.summary_review_desc" => (
            "The run finished. Review the changes from this turn.",
            "运行已完成。请审阅本轮的变更。",
        ),
        "run.footer_completed" => ("Run completed", "运行已完成"),
        "run.completed_desc" => ("The run finished.", "运行已完成。"),
        "run.footer_cancelled" => ("Run cancelled", "运行已取消"),
        "run.cancelled_desc" => (
            "The run was cancelled. Output from this turn is preserved.",
            "运行已取消。本轮的输出已保留。",
        ),
        "run.footer_failed" => ("Run failed", "运行失败"),
        "run.failed_desc_fallback" => ("The run failed.", "运行失败。"),
        // ── Run 状态栏（缺权威来源的字段显示 —，不伪造）──
        "run.status_bar" => (
            "Task — tokens | Quota unavailable | — tok/s | Run {}",
            "任务 — tokens | 配额不可用 | — tok/s | 运行 {}",
        ),
        "run.status_idle" => ("idle", "空闲"),
        // ── Provider 状态文案（render 与 AX 同源；credential 不出现）──
        "provider.auth_connected" => ("Connected", "已连接"),
        "provider.auth_not_connected" => ("Not connected", "未连接"),
        "provider.auth_connecting" => ("Connecting…", "连接中…"),
        "provider.auth_connection_error" => ("Connection error", "连接错误"),
        "provider.catalog_remote" => ("Remote catalog · fetched {}", "远程目录 · 获取于 {}"),
        "provider.catalog_fallback" => {
            ("Built-in catalog fallback · {}", "内置目录回退 · {}")
        }
        "provider.catalog_unavailable_error" => {
            ("Catalog unavailable · {}", "目录不可用 · {}")
        }
        "provider.note_auth_cancelled" => ("Authorization cancelled", "授权已取消"),
        "provider.note_auth_expired" => ("Authorization expired", "授权已过期"),
        "provider.note_connection_removed" => ("Connection removed", "连接已移除"),
        // ── Workspace chrome · 审批卡 ──
        "approval.title" => ("Approval · {}", "审批 · {}"),
        "approval.allow_once" => ("Allow once", "允许一次"),
        "approval.allow_for_run" => ("Allow for run", "允许本次运行"),
        "approval.deny" => ("Deny", "拒绝"),
        "approval.tooltip_allow_once" => (
            "Allow once (Cmd+1 / Cmd+Return)",
            "允许一次（Cmd+1 / Cmd+Return）",
        ),
        "approval.tooltip_allow_for_run" => {
            ("Allow for run (Cmd+2)", "允许本次运行（Cmd+2）")
        }
        "approval.tooltip_deny" => ("Deny (Cmd+3)", "拒绝（Cmd+3）"),
        "approval.disabled_none_pending" => ("No pending approval.", "没有待处理的审批。"),
        "approval.disabled_needs_connection" => {
            ("Approval needs a live connection.", "审批需要有效连接。")
        }
        // ── Workspace chrome · Changes / Activity / Resources / Inspector ──
        "changes.tooltip_refresh" => ("Refresh changes", "刷新变更"),
        "changes.unavailable" => ("Changes unavailable.", "变更不可用。"),
        "changes.unavailable_desc" => (
            "Connect to the Host and open a task to inspect changes.",
            "连接到 Host 并打开任务后，即可查看变更。",
        ),
        "changes.loading" => ("Loading changes…", "正在加载变更…"),
        "changes.loading_desc" => (
            "Reading the latest session diff.",
            "正在读取最新的会话差异。",
        ),
        "changes.no_active_session" => ("No active session.", "没有活动会话。"),
        "changes.no_active_session_desc" => {
            ("Open a task to inspect its changes.", "打开任务后即可查看其变更。")
        }
        "changes.empty" => ("No changes in this session yet.", "本次会话还没有变更。"),
        "changes.empty_desc" => (
            "This session has not reported file changes.",
            "该会话尚未报告文件变更。",
        ),
        "changes.diff_select_file" => {
            ("Select a file to view its diff.", "选择一个文件查看其差异。")
        }
        "changes.diff_select_file_desc" => {
            ("Choose a file above to inspect its diff.", "在上方选择一个文件查看其差异。")
        }
        "changes.diff_loading" => ("Loading diff…", "正在加载差异…"),
        "changes.diff_loading_desc" => {
            ("Reading the selected file diff.", "正在读取所选文件的差异。")
        }
        "changes.diff_binary" => ("Binary file — not rendered.", "二进制文件——不渲染。"),
        "changes.diff_no_hunks" => ("No hunks in response.", "响应中没有差异块。"),
        "changes.error_title" => ("Couldn’t load changes", "无法加载变更"),
        "changes.scope_note" => (
            "Host latest-session diff · workspace context is not a filter",
            "宿主最新会话的差异 · 工作区上下文不是过滤条件",
        ),
        "changes.tab_activity" => ("Activity", "动态"),
        "changes.tab_changes" => ("Changes", "变更"),
        "resources.mcp_title" => ("MCP servers", "MCP 服务器"),
        "resources.tooltip_refresh" => ("Refresh resources", "刷新资源"),
        "resources.unavailable" => ("Resources unavailable.", "资源不可用。"),
        "resources.unavailable_desc" => (
            "Connect to the Host to inspect MCP resources.",
            "连接到 Host 后，即可查看 MCP 资源。",
        ),
        "resources.loading" => ("Loading resources…", "正在加载资源…"),
        "resources.loading_desc" => (
            "Reading the current MCP server list.",
            "正在读取当前 MCP 服务器列表。",
        ),
        "resources.empty" => ("No MCP servers configured.", "尚未配置 MCP 服务器。"),
        "resources.empty_desc" => (
            "No server is available from the current Host.",
            "当前 Host 没有可用的服务器。",
        ),
        "resources.error_title" => ("Couldn’t load resources", "无法加载资源"),
        "common.placeholder_no_details" => {
            ("No additional details are available.", "暂无更多详情。")
        }
        "inspector.tooltip_apply_size" => ("Apply terminal size", "应用终端尺寸"),
        "inspector.tab_changes" => ("Changes", "变更"),
        "inspector.tab_terminal" => ("Terminal", "终端"),
        "inspector.tab_resources" => ("Resources", "资源"),
        "inspector.terminal_empty_output" => {
            ("Terminal output will appear here.", "终端输出将显示在这里。")
        }
        "inspector.resize_not_applied" => ("size not applied", "尺寸尚未应用"),
        "inspector.resize_confirmed" => ("resize confirmed", "尺寸调整已确认"),
        "inspector.terminal_input_placeholder" => (
            "Terminal input… (Enter to write)",
            "终端输入…（Enter 写入）",
        ),
        "header.tooltip_activity" => ("Activity", "动态"),
        // ── Workspace chrome · 状态栏提示 ──
        "status.connect_failed_retry" => (
            "Connect failed. Click Reconnect to retry.",
            "连接失败。点击重新连接重试。",
        ),
        "status.connection_lost" => {
            ("Connection lost. Click Reconnect.", "连接丢失。点击重新连接。")
        }
        "status.project_opened" => ("Project opened · {}", "已打开项目 · {}"),
        "status.forked" => ("Forked · {}", "已分叉 · {}"),
        "status.terminal_create_failed" => ("Create terminal failed: {}", "创建终端失败：{}"),
        "status.terminal_input_sent" => ("Terminal input sent.", "终端输入已发送。"),
        "status.terminal_write_failed" => ("Terminal write failed: {}", "终端写入失败：{}"),
        "status.terminal_size" => ("Terminal size · {}×{}", "终端尺寸 · {}×{}"),
        "status.terminal_resize_failed" => {
            ("Terminal resize failed: {}", "终端尺寸调整失败：{}")
        }
        "status.terminal_closed" => ("Terminal closed.", "终端已关闭。"),
        "status.terminal_close_failed" => ("Terminal close failed: {}", "终端关闭失败：{}"),
        "status.action_failed" => ("{} failed: {}", "{} 失败：{}"),
        "status.open_session_failed" => ("open session failed: {}", "打开会话失败：{}"),
        "status.mcp_remove_failed" => {
            ("Could not remove MCP server · {}", "无法移除 MCP 服务器 · {}")
        }
        "status.mcp_test_failed" => {
            ("Could not test MCP server · {}", "无法测试 MCP 服务器 · {}")
        }
        "status.load_changes_failed" => ("Load changes failed: {}", "加载变更失败：{}"),
        "status.load_diff_failed" => ("Load diff failed: {}", "加载差异失败：{}"),
        "status.load_resources_failed" => ("Load resources failed: {}", "加载资源失败：{}"),
        "status.open_project_needs_connection" => (
            "Opening a project needs a live connection.",
            "打开项目需要有效连接。",
        ),
        "status.choose_project_folder" => ("Choose a project folder…", "选择一个项目文件夹…"),
        "status.opening_project" => ("Opening project…", "正在打开项目…"),
        "status.open_project_cancelled" => ("Open project cancelled.", "已取消打开项目。"),
        "status.open_project_failed" => ("Open project failed: {}", "打开项目失败：{}"),
        "status.no_task_attention" => ("No task needs attention.", "没有需要关注的任务。"),
        "status.changes_not_available" => {
            ("Changes data is not available yet.", "变更数据还不可用。")
        }
        "status.text_scale" => ("Text size · {}%", "字号 · {}%"),
        "status.terminal_needs_connection" => (
            "Terminal needs a live connection; input was kept.",
            "终端需要有效连接；输入已保留。",
        ),
        "status.terminal_waiting_write" => (
            "Waiting for the previous terminal write.",
            "正在等待上一次终端写入。",
        ),
        "status.terminal_not_ready" => (
            "Terminal is not ready; input was kept and nothing was written.",
            "终端尚未就绪；输入已保留，未写入。",
        ),
        "status.terminal_starting" => ("Starting terminal…", "正在启动终端…"),
        "status.open_session_first" => ("Open a session first.", "请先打开一个会话。"),
        "status.language" => ("Language · {}", "语言 · {}"),
        _ => (key, key),
    };
    if lang == Language::Chinese {
        zh
    } else {
        en
    }
}

/// 纯函数：双占位符模板按序替换，不读全局状态。
fn localize_t2(key: &'static str, first: &str, second: &str, lang: Language) -> String {
    localize(key, lang)
        .replacen("{}", first, 1)
        .replacen("{}", second, 1)
}

/// 纯函数：目录概览文案。English 按可数名词处理单复数，中文用量词。
fn localize_catalog_overview(model_count: usize, lang: Language) -> String {
    if model_count == 0 {
        return localize("settings.providers.catalog_available", lang).to_string();
    }
    if lang == Language::Chinese {
        format!("{model_count} 个模型")
    } else {
        format!(
            "{model_count} model{}",
            if model_count == 1 { "" } else { "s" }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认 English 是既有测试与 AX 断言的前提；显式钉住。
    /// 测试不写全局 language：避免与并行 bin 测试经 t() 竞态。
    #[test]
    fn default_language_is_english() {
        assert_eq!(language(), Language::English);
        assert_eq!(t("settings.appearance.language"), "Language");
    }

    /// 纯函数覆盖两种语言与未知 key 回退（漏译可见）；数据内容不经 t()，不受影响。
    #[test]
    fn localize_covers_both_languages_and_unknown_key() {
        assert_eq!(
            localize("settings.appearance.language", Language::English),
            "Language"
        );
        assert_eq!(
            localize("settings.appearance.language", Language::Chinese),
            "语言"
        );
        assert_eq!(localize("no.such.key", Language::Chinese), "no.such.key");
    }

    /// AX press 派发的反查映射：两个语言 id 往返一致，未知 id fail-closed。
    /// 纯函数断言，不写全局语言（与并行 bin 测试的 t() 竞态约束一致）。
    #[test]
    fn language_from_identifier_round_trips_and_rejects_unknown() {
        for language in LANGUAGES {
            assert_eq!(language_from_identifier(language.identifier()), Some(language));
        }
        assert_eq!(language_from_identifier("settings-language-fr"), None);
        assert_eq!(language_from_identifier("settings-text-scale-100"), None);
    }
}
