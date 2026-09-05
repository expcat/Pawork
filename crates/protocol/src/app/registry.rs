//! Command/Capability Registry：AppCommand/AppQuery 的单一授权事实源。
//!
//! R3 波 A 落地：wire 名、通道可用性（GUI / headless / ACP）、所需能力、
//! 幂等性质与引入协议版本集中在本表。GUI 通道的 wire 名解析、能力宣告与
//! 逐命令授权均从 registry 派生；未登记 wire 名 fail-closed。headless 与
//! ACP 消费侧切换在波 B 完成（本波只登记数据，不触碰两通道实现）。

use crate::headless::wire::SdkCapability;
use crate::GuiCapability;

use super::command::AppCommand;
use super::query::AppQuery;
use super::version::{ApiVersion, V1_0, V1_1, V1_2, V1_3, V1_4, V1_5, V1_6, V1_7, V1_8, V1_10, V1_11};

/// GUI 通道访问规格：是否可用 + 命令级所需能力。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuiChannelAccess {
    /// 该条目是否允许经 GUI 通道进入 host。false 表示通道级拒绝
    /// （如 host 专用命令），授权门在进入 host 前拒绝。
    pub available: bool,
    /// 执行所需的最小 GUI capability；None 表示无命令级能力要求。
    pub required_capability: Option<GuiCapability>,
}

/// 一条 AppCommand/AppQuery 的登记（表驱动，纯数据）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryEntry {
    /// 与 serde tag（method，snake_case）一致的 wire 名。
    pub wire_name: &'static str,
    /// GUI 通道可用性与命令级能力要求。
    pub gui: GuiChannelAccess,
    /// headless 通道所需 SdkCapability；None = 未映射，授权 fail-closed
    /// （与 headless 现行 gate_capability 语义一致；波 B 切换消费）。
    pub headless: Option<SdkCapability>,
    /// 是否可经 ACP method 白名单触达（波 B 切换消费）。
    pub acp: bool,
    /// 幂等性质：同参数重复执行是否收敛到同一状态（查询恒为 true）。
    pub idempotent: bool,
    /// 该条目当前契约形状引入的协议版本（见 version.rs 注释）。
    pub since: ApiVersion,
}

/// GUI 通道内禀能力：连接层事件订阅与快照恢复，不挂接具体命令。
///
/// ArtifactStreaming 不在内禀集也无任何条目 require
/// （K-08 / R0 D13：双端从未实现，已停止宣告，枚举冻结保留）。
pub const GUI_INTRINSIC_CAPABILITIES: &[GuiCapability] =
    &[GuiCapability::Events, GuiCapability::Snapshots];

static COMMANDS: &[RegistryEntry] = &[
    // --- AppCommand（29）---
    RegistryEntry {
        wire_name: "core_initialize",
        gui: GuiChannelAccess {
            available: false,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "workspace_add",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: false,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "workspace_trust",
        // ADR-048 D3：GUI 开放（会话内信任切换，不写盘）；可用性变化随
        // 1.6 生效，since 维持词汇首次登记的 V1_0。
        gui: GuiChannelAccess {
            available: true,
            required_capability: Some(GuiCapability::Approvals),
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "session_create",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: Some(SdkCapability::Sessions),
        acp: true,
        idempotent: false,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "session_open",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: Some(SdkCapability::Sessions),
        acp: false,
        idempotent: true,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "session_fork",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: Some(SdkCapability::Sessions),
        acp: false,
        idempotent: false,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "session_compact",
        gui: GuiChannelAccess {
            available: false,
            required_capability: None,
        },
        headless: Some(SdkCapability::Sessions),
        acp: false,
        idempotent: false,
        since: V1_0,
    },
    // ADR-054 OPT-2（since 1.11）：会话改名 / 归档，Desktop 入口；
    // headless / ACP 暂不映射（fail-closed）。
    RegistryEntry {
        wire_name: "session_rename",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_11,
    },
    RegistryEntry {
        wire_name: "session_archive",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_11,
    },
    // Host（IDE/ACP）侧命令：GUI 通道维持 S7 起的 PermissionDenied 拒绝，
    // 不进入 GuiHost；headless 侧按 Sessions 能力映射（波 B 消费）。
    RegistryEntry {
        wire_name: "session_client_context_replace",
        gui: GuiChannelAccess {
            available: false,
            required_capability: None,
        },
        headless: Some(SdkCapability::Sessions),
        acp: false,
        idempotent: true,
        since: V1_0,
    },
    // RunStart.provider 字段随 1.2 引入（version.rs），条目形状记 1.2。
    RegistryEntry {
        wire_name: "run_start",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: Some(SdkCapability::Runs),
        acp: true,
        idempotent: false,
        since: V1_2,
    },
    RegistryEntry {
        wire_name: "run_cancel",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: Some(SdkCapability::Runs),
        acp: true,
        idempotent: true,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "run_retry",
        gui: GuiChannelAccess {
            available: false,
            required_capability: None,
        },
        headless: Some(SdkCapability::Runs),
        acp: false,
        idempotent: false,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "run_tool",
        gui: GuiChannelAccess {
            available: false,
            required_capability: None,
        },
        headless: Some(SdkCapability::Runs),
        acp: false,
        idempotent: false,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "auth_start",
        // SET-1（ADR-046）：Provider 认证流改由 GUI 触发；GUI 语义由
        // ADR-046 D3 在 1.4 锁定，since 记为该语义引入版本。
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: false,
        since: V1_4,
    },
    RegistryEntry {
        wire_name: "auth_remove",
        // SET-1（ADR-046）：Provider 认证流改由 GUI 触发；GUI 语义由
        // ADR-046 D3 在 1.4 锁定，since 记为该语义引入版本。
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_4,
    },
    RegistryEntry {
        wire_name: "auth_set_api_key",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_4,
    },
    RegistryEntry {
        wire_name: "auth_cancel",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_4,
    },
    RegistryEntry {
        wire_name: "set_default_model",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_4,
    },
    RegistryEntry {
        wire_name: "set_proxy_url",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_5,
    },
    // ADR-052 SET-6h：供应商级代理开关（Global 原子写 + 内存同步）；
    // 仅 GUI 开放，未知 provider 宿主侧 fail-closed 报错。
    RegistryEntry {
        wire_name: "set_provider_use_proxy",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_10,
    },
    RegistryEntry {
        wire_name: "set_approval_mode",
        // ADR-053：保存 Global 审批默认；仅 GUI 开放。
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_6,
    },
    // ADR-050 D3：终端默认值全态写（Global 原子写 + 内存同步）；
    // 仅 GUI 开放，非法 shell/越界尺寸宿主侧 fail-closed 保旧。
    RegistryEntry {
        wire_name: "set_terminal_settings",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_8,
    },
    RegistryEntry {
        wire_name: "tool_approve",
        gui: GuiChannelAccess {
            available: true,
            required_capability: Some(GuiCapability::Approvals),
        },
        headless: Some(SdkCapability::Runs),
        acp: true,
        idempotent: true,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "git_stage",
        gui: GuiChannelAccess {
            available: false,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "terminal_create",
        gui: GuiChannelAccess {
            available: true,
            required_capability: Some(GuiCapability::TerminalStreaming),
        },
        headless: None,
        acp: false,
        idempotent: false,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "terminal_write",
        gui: GuiChannelAccess {
            available: true,
            required_capability: Some(GuiCapability::TerminalStreaming),
        },
        headless: None,
        acp: false,
        idempotent: false,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "terminal_resize",
        gui: GuiChannelAccess {
            available: true,
            required_capability: Some(GuiCapability::TerminalStreaming),
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_0,
    },
    // ADR-045：终止并注销终端会话（重复 close 报 not_found，非幂等）。
    RegistryEntry {
        wire_name: "terminal_close",
        gui: GuiChannelAccess {
            available: true,
            required_capability: Some(GuiCapability::TerminalStreaming),
        },
        headless: None,
        acp: false,
        idempotent: false,
        since: V1_3,
    },
    // ADR-049 D1：MCP 现场验证（触网、可能改变连接状态）；仅 GUI 开放。
    RegistryEntry {
        wire_name: "mcp_test",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: false,
        since: V1_7,
    },
    // ADR-049 D2：Global 层移除 MCP server（写盘/清密/内存同步）；
    // 仅 GUI 开放，未知 name fail-closed 保旧。
    RegistryEntry {
        wire_name: "mcp_server_remove",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: false,
        since: V1_7,
    },
];

static QUERIES: &[RegistryEntry] = &[
    // --- AppQuery（15）---
    RegistryEntry {
        wire_name: "workspace_list",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_0,
    },
    // Timeline 分页字段随 1.1 引入（version.rs），条目形状记 1.1。
    RegistryEntry {
        wire_name: "session_get",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: Some(SdkCapability::Sessions),
        acp: false,
        idempotent: true,
        since: V1_1,
    },
    RegistryEntry {
        wire_name: "run_status",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: Some(SdkCapability::Runs),
        acp: false,
        idempotent: true,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "model_list",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "diff_list_files",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "diff_get",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_0,
    },
    // 专用 ArtifactRead 帧尚未接线；AppQuery 变体也未由 GuiHost 实现。
    RegistryEntry {
        wire_name: "artifact_read",
        gui: GuiChannelAccess {
            available: false,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "quota_overview",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "snapshot_fetch",
        gui: GuiChannelAccess {
            // GUI 使用专用 SnapshotRequest 帧；AppQuery 变体未由 GuiHost 实现。
            available: false,
            required_capability: Some(GuiCapability::Snapshots),
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "plugin_list",
        gui: GuiChannelAccess {
            available: false,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "mcp_list",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_0,
    },
    RegistryEntry {
        wire_name: "provider_auth_status",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_4,
    },
    RegistryEntry {
        wire_name: "general_settings",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_5,
    },
    RegistryEntry {
        wire_name: "permissions_settings",
        // ADR-048 D1：审批模式 / 会话信任 / Global 信任三元组；仅 GUI 开放。
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_6,
    },
    // ADR-050 D2：终端默认设置查询（shell 持久值 + columns/rows 生效值）；
    // 仅 GUI 开放。
    RegistryEntry {
        wire_name: "terminal_settings",
        gui: GuiChannelAccess {
            available: true,
            required_capability: None,
        },
        headless: None,
        acp: false,
        idempotent: true,
        since: V1_8,
    },
];

/// 变体 → wire 名的唯一映射（从 gui_host 平移收编；禁止在通道侧再建镜像）。
pub fn command_wire_name(command: &AppCommand) -> &'static str {
    match command {
        AppCommand::CoreInitialize => "core_initialize",
        AppCommand::WorkspaceAdd { .. } => "workspace_add",
        AppCommand::WorkspaceTrust { .. } => "workspace_trust",
        AppCommand::SessionCreate { .. } => "session_create",
        AppCommand::SessionOpen { .. } => "session_open",
        AppCommand::SessionFork { .. } => "session_fork",
        AppCommand::SessionCompact { .. } => "session_compact",
        AppCommand::SessionRename { .. } => "session_rename",
        AppCommand::SessionArchive { .. } => "session_archive",
        AppCommand::SessionClientContextReplace { .. } => "session_client_context_replace",
        AppCommand::RunStart { .. } => "run_start",
        AppCommand::RunCancel { .. } => "run_cancel",
        AppCommand::RunRetry { .. } => "run_retry",
        AppCommand::RunTool { .. } => "run_tool",
        AppCommand::AuthStart { .. } => "auth_start",
        AppCommand::AuthRemove { .. } => "auth_remove",
        AppCommand::AuthSetApiKey { .. } => "auth_set_api_key",
        AppCommand::AuthCancel { .. } => "auth_cancel",
        AppCommand::SetDefaultModel { .. } => "set_default_model",
        AppCommand::SetProxyUrl { .. } => "set_proxy_url",
        AppCommand::SetProviderUseProxy { .. } => "set_provider_use_proxy",
        AppCommand::SetApprovalMode { .. } => "set_approval_mode",
        AppCommand::SetTerminalSettings { .. } => "set_terminal_settings",
        AppCommand::ToolApprove { .. } => "tool_approve",
        AppCommand::GitStage { .. } => "git_stage",
        AppCommand::TerminalCreate { .. } => "terminal_create",
        AppCommand::TerminalWrite { .. } => "terminal_write",
        AppCommand::TerminalResize { .. } => "terminal_resize",
        AppCommand::TerminalClose { .. } => "terminal_close",
        AppCommand::McpTest { .. } => "mcp_test",
        AppCommand::McpServerRemove { .. } => "mcp_server_remove",
    }
}

/// 变体 → wire 名的唯一映射（AppQuery 侧）。
pub fn query_wire_name(query: &AppQuery) -> &'static str {
    match query {
        AppQuery::WorkspaceList => "workspace_list",
        AppQuery::SessionGet { .. } => "session_get",
        AppQuery::RunStatus { .. } => "run_status",
        AppQuery::ModelList { .. } => "model_list",
        AppQuery::DiffListFiles { .. } => "diff_list_files",
        AppQuery::DiffGet { .. } => "diff_get",
        AppQuery::ArtifactRead { .. } => "artifact_read",
        AppQuery::QuotaOverview { .. } => "quota_overview",
        AppQuery::SnapshotFetch => "snapshot_fetch",
        AppQuery::PluginList => "plugin_list",
        AppQuery::McpList => "mcp_list",
        AppQuery::ProviderAuthStatus { .. } => "provider_auth_status",
        AppQuery::GeneralSettings => "general_settings",
        AppQuery::PermissionsSettings => "permissions_settings",
        AppQuery::TerminalSettings => "terminal_settings",
    }
}

/// 按 wire 名查 AppCommand 登记；未登记 fail-closed（返回 None）。
pub fn command_by_wire_name(wire_name: &str) -> Option<&'static RegistryEntry> {
    COMMANDS.iter().find(|entry| entry.wire_name == wire_name)
}

/// 按 wire 名查 AppQuery 登记；未登记 fail-closed（返回 None）。
pub fn query_by_wire_name(wire_name: &str) -> Option<&'static RegistryEntry> {
    QUERIES.iter().find(|entry| entry.wire_name == wire_name)
}

/// AppCommand 变体的登记条目。
///
/// 变体集合由 command_wire_name 的穷尽 match 与完整性测试共同钉死；
/// 表缺失条目属编程错误，直接 panic（fail-fast，不静默放行）。
pub fn command_entry(command: &AppCommand) -> &'static RegistryEntry {
    let wire_name = command_wire_name(command);
    command_by_wire_name(wire_name)
        .unwrap_or_else(|| panic!("command registry missing entry for {wire_name}"))
}

/// AppQuery 变体的登记条目（语义同 command_entry）。
pub fn query_entry(query: &AppQuery) -> &'static RegistryEntry {
    let wire_name = query_wire_name(query);
    query_by_wire_name(wire_name)
        .unwrap_or_else(|| panic!("query registry missing entry for {wire_name}"))
}

/// 全量 AppCommand 登记表（按 enum 声明序）。
pub fn command_entries() -> &'static [RegistryEntry] {
    COMMANDS
}

/// 全量 AppQuery 登记表（按 enum 声明序）。
pub fn query_entries() -> &'static [RegistryEntry] {
    QUERIES
}

fn capability_rank(capability: &GuiCapability) -> u8 {
    match capability {
        GuiCapability::Events => 0,
        GuiCapability::Snapshots => 1,
        // 无任何条目 require ArtifactStreaming（K-08 / R0 D13）。
        GuiCapability::ArtifactStreaming => 2,
        GuiCapability::TerminalStreaming => 3,
        GuiCapability::Approvals => 4,
    }
}

/// 派生 GUI 服务端宣告集：GUI 可用条目 require 的能力 ∪ 通道内禀能力
/// （Events / Snapshots）。按 GuiCapability 声明序输出，保证向量稳定。
///
/// 波 A 基线：派生结果必须等于 V2 快照
/// {Events, Snapshots, TerminalStreaming, Approvals}（golden 测试钉死）。
pub fn gui_supported_capabilities() -> Vec<GuiCapability> {
    let mut capabilities = GUI_INTRINSIC_CAPABILITIES.to_vec();
    for entry in COMMANDS.iter().chain(QUERIES.iter()) {
        if let (true, Some(capability)) =
            (entry.gui.available, entry.gui.required_capability.as_ref())
        {
            if !capabilities.contains(capability) {
                capabilities.push(capability.clone());
            }
        }
    }
    capabilities.sort_by_key(capability_rank);
    capabilities
}
