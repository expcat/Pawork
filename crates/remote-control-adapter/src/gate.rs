//! 受限协议门禁：canonical 命令/查询的穷举分类。
//!
//! **fail-closed**：分类使用穷举 match（无通配分支）；canonical
//! `AppCommand` / `AppQuery` 一旦新增变体，本 crate 编译失败，强制为新
//! 操作显式登记分类，杜绝"默认放行"。
//!
//! 允许集（P17-12）：
//!
//! - 命令：`RunStart` / `RunCancel` / `ToolApprove`；
//! - 查询：`SessionGet` / `RunStatus`（计划状态查询在服务层经
//!   `SessionGet` 代理，见 `service` 模块）。

use core_api::{AppCommand, AppQuery};

/// 拒绝码：直接执行工具（RunTool）。
pub const DENY_TOOL_EXECUTION: &str = "tool_execution_denied";
/// 拒绝码：文件/宿主写入类（GitStage、终端创建/写入/改尺寸）。
pub const DENY_FILE_WRITE: &str = "file_write_denied";
/// 拒绝码：Provider 直连 / 凭据管理 / 模型目录。
pub const DENY_PROVIDER_DIRECT_ACCESS: &str = "provider_direct_access_denied";
/// 拒绝码：会话结构与上下文变更。
pub const DENY_SESSION_MUTATION: &str = "session_mutation_denied";
/// 拒绝码：工作区注册与信任变更。
pub const DENY_WORKSPACE_MUTATION: &str = "workspace_mutation_denied";
/// 拒绝码：Host 生命周期变更（CoreInitialize）。
pub const DENY_HOST_MUTATION: &str = "host_mutation_denied";
/// 拒绝码：批量/内容读取（diff、artifact、快照、配额总览、列表）。
pub const DENY_CONTENT_READ: &str = "content_read_denied";
/// 拒绝码：未列入远程允许集的其他 canonical 操作（如 RunRetry）。
pub const DENY_NOT_EXPOSED: &str = "operation_not_exposed";

/// 门禁裁决。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// 允许映射到 Core。
    Allow,
    /// 显式拒绝（附稳定拒绝码与人类可读原因；必须落审计）。
    Deny {
        code: &'static str,
        reason: &'static str,
    },
}

impl Verdict {
    pub fn is_allowed(self) -> bool {
        matches!(self, Verdict::Allow)
    }

    pub fn deny_code(self) -> Option<&'static str> {
        match self {
            Verdict::Allow => None,
            Verdict::Deny { code, .. } => Some(code),
        }
    }
}

/// 命令分类：仅 `RunStart` / `RunCancel` / `ToolApprove` 放行。
pub fn classify_command(command: &AppCommand) -> Verdict {
    match command {
        AppCommand::RunStart { .. }
        | AppCommand::RunCancel { .. }
        | AppCommand::ToolApprove { .. } => Verdict::Allow,
        AppCommand::RunTool { .. } => Verdict::Deny {
            code: DENY_TOOL_EXECUTION,
            reason: "远程通道不允许直接执行工具",
        },
        AppCommand::GitStage { .. } => Verdict::Deny {
            code: DENY_FILE_WRITE,
            reason: "远程通道不允许写 Git 暂存区",
        },
        AppCommand::TerminalCreate { .. }
        | AppCommand::TerminalWrite { .. }
        | AppCommand::TerminalResize { .. } => Verdict::Deny {
            code: DENY_FILE_WRITE,
            reason: "终端可在宿主任意执行/写入，远程通道不允许",
        },
        AppCommand::AuthStart { .. } | AppCommand::AuthRemove { .. } => Verdict::Deny {
            code: DENY_PROVIDER_DIRECT_ACCESS,
            reason: "Provider 凭据仅宿主侧管理，远程不允许直连或变更凭据",
        },
        AppCommand::SessionCreate { .. }
        | AppCommand::SessionOpen { .. }
        | AppCommand::SessionFork { .. }
        | AppCommand::SessionCompact { .. }
        | AppCommand::SessionClientContextReplace { .. } => Verdict::Deny {
            code: DENY_SESSION_MUTATION,
            reason: "会话结构与上下文变更不在远程允许集",
        },
        AppCommand::WorkspaceAdd { .. } | AppCommand::WorkspaceTrust { .. } => Verdict::Deny {
            code: DENY_WORKSPACE_MUTATION,
            reason: "工作区注册与信任变更不在远程允许集",
        },
        AppCommand::CoreInitialize => Verdict::Deny {
            code: DENY_HOST_MUTATION,
            reason: "Host 生命周期命令不在远程允许集",
        },
        AppCommand::RunRetry { .. } => Verdict::Deny {
            code: DENY_NOT_EXPOSED,
            reason: "RunRetry 未列入远程允许集",
        },
    }
}

/// 查询分类：仅 `SessionGet` / `RunStatus` 放行。
pub fn classify_query(query: &AppQuery) -> Verdict {
    match query {
        AppQuery::SessionGet { .. } | AppQuery::RunStatus { .. } => Verdict::Allow,
        AppQuery::ModelList { .. } => Verdict::Deny {
            code: DENY_PROVIDER_DIRECT_ACCESS,
            reason: "模型/Provider 目录不对远程暴露",
        },
        AppQuery::DiffListFiles { .. }
        | AppQuery::DiffGet { .. }
        | AppQuery::ArtifactRead { .. }
        | AppQuery::SnapshotFetch
        | AppQuery::QuotaOverview { .. } => Verdict::Deny {
            code: DENY_CONTENT_READ,
            reason: "批量/内容读取不对远程暴露",
        },
        AppQuery::WorkspaceList | AppQuery::PluginList | AppQuery::McpList => Verdict::Deny {
            code: DENY_CONTENT_READ,
            reason: "清单类读取不对远程暴露",
        },
    }
}

/// 命令的稳定操作名（审计/拒绝帧使用）。
pub fn command_operation(command: &AppCommand) -> &'static str {
    match command {
        AppCommand::CoreInitialize => "core_initialize",
        AppCommand::WorkspaceAdd { .. } => "workspace_add",
        AppCommand::WorkspaceTrust { .. } => "workspace_trust",
        AppCommand::SessionCreate { .. } => "session_create",
        AppCommand::SessionOpen { .. } => "session_open",
        AppCommand::SessionFork { .. } => "session_fork",
        AppCommand::SessionCompact { .. } => "session_compact",
        AppCommand::SessionClientContextReplace { .. } => "session_client_context_replace",
        AppCommand::RunStart { .. } => "run_start",
        AppCommand::RunCancel { .. } => "run_cancel",
        AppCommand::RunRetry { .. } => "run_retry",
        AppCommand::RunTool { .. } => "run_tool",
        AppCommand::AuthStart { .. } => "auth_start",
        AppCommand::AuthRemove { .. } => "auth_remove",
        AppCommand::ToolApprove { .. } => "tool_approve",
        AppCommand::GitStage { .. } => "git_stage",
        AppCommand::TerminalCreate { .. } => "terminal_create",
        AppCommand::TerminalWrite { .. } => "terminal_write",
        AppCommand::TerminalResize { .. } => "terminal_resize",
    }
}

/// 查询的稳定操作名（审计/拒绝帧使用）。
pub fn query_operation(query: &AppQuery) -> &'static str {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{ArtifactId, ProviderId, RunId, SessionId, ToolCallId, WorkspaceId};
    use core_api::{ApprovalDecision, QuotaOverviewQuery};
    use serde_json::json;

    /// 构造所有 canonical 命令变体（穷举；新增变体时编译失败强制登记）。
    fn all_commands() -> Vec<(AppCommand, &'static str)> {
        vec![
            (AppCommand::CoreInitialize, "host_mutation_denied"),
            (
                AppCommand::WorkspaceAdd {
                    root_path: "/tmp/x".into(),
                },
                "workspace_mutation_denied",
            ),
            (
                AppCommand::WorkspaceTrust {
                    workspace_id: WorkspaceId::from("w"),
                    trusted: true,
                },
                "workspace_mutation_denied",
            ),
            (
                AppCommand::SessionCreate {
                    workspace_id: WorkspaceId::from("w"),
                    title: None,
                },
                "session_mutation_denied",
            ),
            (
                AppCommand::SessionOpen {
                    session_id: SessionId::from("s"),
                },
                "session_mutation_denied",
            ),
            (
                AppCommand::SessionFork {
                    session_id: SessionId::from("s"),
                    parent_event_id: agent_domain::EventId::from("e"),
                },
                "session_mutation_denied",
            ),
            (
                AppCommand::SessionCompact {
                    session_id: SessionId::from("s"),
                },
                "session_mutation_denied",
            ),
            (
                AppCommand::SessionClientContextReplace {
                    session_id: SessionId::from("s"),
                    snapshot: core_api::ClientContextSnapshot {
                        revision: 1,
                        active_document: None,
                        open_documents: Vec::new(),
                        diagnostics: Vec::new(),
                    },
                },
                "session_mutation_denied",
            ),
            (
                AppCommand::RunStart {
                    session_id: SessionId::from("s"),
                    user_message: "hi".into(),
                    model: None,
                    profile: None,
                },
                "",
            ),
            (
                AppCommand::RunCancel {
                    run_id: RunId::from("r"),
                },
                "",
            ),
            (
                AppCommand::RunRetry {
                    run_id: RunId::from("r"),
                },
                "operation_not_exposed",
            ),
            (
                AppCommand::RunTool {
                    run_id: RunId::from("r"),
                    tool_name: "shell".into(),
                    input: json!({}),
                },
                "tool_execution_denied",
            ),
            (
                AppCommand::AuthStart {
                    provider_id: ProviderId::from("p"),
                    flow: "api_key".into(),
                },
                "provider_direct_access_denied",
            ),
            (
                AppCommand::AuthRemove {
                    provider_id: ProviderId::from("p"),
                },
                "provider_direct_access_denied",
            ),
            (
                AppCommand::ToolApprove {
                    run_id: RunId::from("r"),
                    tool_call_id: ToolCallId::from("t"),
                    decision: ApprovalDecision::ApproveOnce,
                },
                "",
            ),
            (
                AppCommand::GitStage {
                    workspace_id: WorkspaceId::from("w"),
                    paths: Vec::new(),
                },
                "file_write_denied",
            ),
            (
                AppCommand::TerminalCreate {
                    workspace_id: WorkspaceId::from("w"),
                    working_directory: None,
                },
                "file_write_denied",
            ),
            (
                AppCommand::TerminalWrite {
                    terminal_session_id: "t".into(),
                    data: "x".into(),
                },
                "file_write_denied",
            ),
            (
                AppCommand::TerminalResize {
                    terminal_session_id: "t".into(),
                    columns: 80,
                    rows: 24,
                },
                "file_write_denied",
            ),
        ]
    }

    #[test]
    fn allowlist_is_exactly_run_start_cancel_and_tool_approve() {
        let allowed: Vec<&'static str> = all_commands()
            .into_iter()
            .filter(|(command, _)| classify_command(command).is_allowed())
            .map(|(_, _)| "")
            .collect();
        assert_eq!(allowed.len(), 3, "允许集必须恰好为 3 条命令");
    }

    #[test]
    fn every_non_allowed_command_is_denied_with_expected_code() {
        for (command, expected) in all_commands() {
            let verdict = classify_command(&command);
            let operation = command_operation(&command);
            assert!(!operation.is_empty(), "每个变体必须有操作名");
            if expected.is_empty() {
                assert_eq!(verdict, Verdict::Allow, "{operation} 应在允许集");
            } else {
                assert_eq!(
                    verdict.deny_code(),
                    Some(expected),
                    "{operation} 拒绝码不符"
                );
            }
        }
    }

    #[test]
    fn query_allowlist_is_exactly_session_get_and_run_status() {
        let queries: Vec<(AppQuery, &'static str)> = vec![
            (AppQuery::WorkspaceList, "content_read_denied"),
            (
                AppQuery::SessionGet {
                    session_id: SessionId::from("s"),
                },
                "",
            ),
            (
                AppQuery::RunStatus {
                    run_id: RunId::from("r"),
                },
                "",
            ),
            (
                AppQuery::ModelList { provider_id: None },
                "provider_direct_access_denied",
            ),
            (
                AppQuery::DiffListFiles {
                    workspace_id: WorkspaceId::from("w"),
                },
                "content_read_denied",
            ),
            (
                AppQuery::DiffGet {
                    workspace_id: WorkspaceId::from("w"),
                    path: core_api::WorkspaceRelativePath::new("f.txt").expect("relative path"),
                    cursor: None,
                },
                "content_read_denied",
            ),
            (
                AppQuery::ArtifactRead {
                    artifact_id: ArtifactId::from("a"),
                    offset: 0,
                    limit: 16,
                },
                "content_read_denied",
            ),
            (
                AppQuery::QuotaOverview {
                    query: QuotaOverviewQuery::default_local(),
                },
                "content_read_denied",
            ),
            (AppQuery::SnapshotFetch, "content_read_denied"),
            (AppQuery::PluginList, "content_read_denied"),
            (AppQuery::McpList, "content_read_denied"),
        ];
        let mut allowed = 0usize;
        for (query, expected) in queries {
            let verdict = classify_query(&query);
            let operation = query_operation(&query);
            assert!(!operation.is_empty());
            if expected.is_empty() {
                assert_eq!(verdict, Verdict::Allow, "{operation} 应在允许集");
                allowed += 1;
            } else {
                assert_eq!(
                    verdict.deny_code(),
                    Some(expected),
                    "{operation} 拒绝码不符"
                );
            }
        }
        assert_eq!(allowed, 2, "查询允许集必须恰好为 SessionGet/RunStatus");
    }
}
