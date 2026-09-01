//! Command/Capability Registry 定向测试（R3 波 A，golden 先行）。
//!
//! 结构保证：穷尽无通配符 match 覆盖全部 AppCommand/AppQuery 变体，
//! 新增变体时本文件编译失败，强制补齐 registry 登记；wire 名与 serde
//! tag 的双射由逐变体样本 round-trip 钉死；GUI 宣告向量按 V2 快照冻结。

use pawork_protocol::app::registry::{
    command_by_wire_name, command_entries, command_entry, command_wire_name,
    gui_supported_capabilities, query_by_wire_name, query_entries, query_entry, query_wire_name,
};
use pawork_protocol::headless::wire::SdkCapability;
use pawork_protocol::{ApiVersion, AppCommand, AppQuery, GuiCapability};
use serde_json::{json, Value};

const V1_0: ApiVersion = ApiVersion { major: 1, minor: 0 };
const V1_1: ApiVersion = ApiVersion { major: 1, minor: 1 };
const V1_2: ApiVersion = ApiVersion { major: 1, minor: 2 };
const V1_3: ApiVersion = ApiVersion { major: 1, minor: 3 };
const V1_4: ApiVersion = ApiVersion { major: 1, minor: 4 };

/// (wire 名, 最小 params 样本；None = unit 变体无 params)。
fn command_samples() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("core_initialize", None),
        ("workspace_add", Some(json!({"root_path": "/tmp/demo"}))),
        (
            "workspace_trust",
            Some(json!({"workspace_id": "ws-1", "trusted": true})),
        ),
        ("session_create", Some(json!({"workspace_id": "ws-1"}))),
        ("session_open", Some(json!({"session_id": "session-1"}))),
        (
            "session_fork",
            Some(json!({"session_id": "session-1", "parent_event_id": "event-1"})),
        ),
        ("session_compact", Some(json!({"session_id": "session-1"}))),
        (
            "session_client_context_replace",
            Some(json!({"session_id": "session-1", "snapshot": {"revision": 1}})),
        ),
        (
            "run_start",
            Some(json!({"session_id": "session-1", "user_message": "hi"})),
        ),
        ("run_cancel", Some(json!({"run_id": "run-1"}))),
        ("run_retry", Some(json!({"run_id": "run-1"}))),
        (
            "run_tool",
            Some(json!({"run_id": "run-1", "tool_name": "shell", "input": {}})),
        ),
        (
            "auth_start",
            Some(json!({"provider_id": "glm-coding", "flow": "oauth"})),
        ),
        ("auth_remove", Some(json!({"provider_id": "glm-coding"}))),
        (
            "auth_set_api_key",
            Some(json!({"provider_id": "glm-coding", "api_key": "sk-test-fixture"})),
        ),
        ("auth_cancel", Some(json!({"provider_id": "glm-coding"}))),
        (
            "set_default_model",
            Some(json!({"provider_id": "glm-coding", "model_id": "glm-4.7"})),
        ),
        (
            "tool_approve",
            Some(json!({
                "run_id": "run-1",
                "tool_call_id": "call-1",
                "decision": "approve_once"
            })),
        ),
        (
            "git_stage",
            Some(json!({"workspace_id": "ws-1", "paths": ["a.rs"]})),
        ),
        ("terminal_create", Some(json!({"workspace_id": "ws-1"}))),
        (
            "terminal_write",
            Some(json!({"terminal_session_id": "t-1", "data": "ls"})),
        ),
        (
            "terminal_resize",
            Some(json!({"terminal_session_id": "t-1", "columns": 80, "rows": 24})),
        ),
        (
            "terminal_close",
            Some(json!({"terminal_session_id": "t-1"})),
        ),
    ]
}

fn query_samples() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("workspace_list", None),
        ("session_get", Some(json!({"session_id": "session-1"}))),
        ("run_status", Some(json!({"run_id": "run-1"}))),
        ("model_list", Some(json!({}))),
        ("diff_list_files", Some(json!({"workspace_id": "ws-1"}))),
        (
            "diff_get",
            Some(json!({"workspace_id": "ws-1", "path": "a.rs"})),
        ),
        (
            "artifact_read",
            Some(json!({"artifact_id": "artifact-1", "offset": 0, "limit": 16})),
        ),
        (
            "quota_overview",
            Some(json!({"query": {"tenant_id": "local/default", "account_id": "local/default"}})),
        ),
        ("snapshot_fetch", None),
        ("plugin_list", None),
        ("mcp_list", None),
        (
            "provider_auth_status",
            Some(json!({"provider_id": "glm-coding"})),
        ),
    ]
}

fn wire_frame(wire_name: &str, params: &Option<Value>) -> Value {
    let mut frame = json!({"method": wire_name});
    if let Some(params) = params {
        frame["params"] = params.clone();
    }
    frame
}

fn sample_commands() -> Vec<AppCommand> {
    command_samples()
        .iter()
        .map(|(wire_name, params)| {
            serde_json::from_value(wire_frame(wire_name, params)).expect("sample decodes")
        })
        .collect()
}

fn sample_queries() -> Vec<AppQuery> {
    query_samples()
        .iter()
        .map(|(wire_name, params)| {
            serde_json::from_value(wire_frame(wire_name, params)).expect("sample decodes")
        })
        .collect()
}

#[test]
fn wire_names_are_bijective_with_serde_tags() {
    for (wire_name, params) in command_samples() {
        let frame = wire_frame(wire_name, &params);
        let command: AppCommand = serde_json::from_value(frame.clone()).expect("decode");
        assert_eq!(command_wire_name(&command), wire_name);
        assert_eq!(
            command_by_wire_name(wire_name)
                .expect("command registered")
                .wire_name,
            wire_name
        );
        assert_eq!(serde_json::to_value(&command).expect("encode"), frame);
    }
    for (wire_name, params) in query_samples() {
        let frame = wire_frame(wire_name, &params);
        let query: AppQuery = serde_json::from_value(frame.clone()).expect("decode");
        assert_eq!(query_wire_name(&query), wire_name);
        assert_eq!(
            query_by_wire_name(wire_name)
                .expect("query registered")
                .wire_name,
            wire_name
        );
        assert_eq!(serde_json::to_value(&query).expect("encode"), frame);
    }
}

#[test]
fn registry_tables_are_complete_and_unique() {
    let commands = command_entries();
    let queries = query_entries();
    assert_eq!(commands.len(), 23);
    assert_eq!(queries.len(), 12);
    for wire_name in commands.iter().map(|entry| entry.wire_name) {
        assert_eq!(
            commands
                .iter()
                .filter(|entry| entry.wire_name == wire_name)
                .count(),
            1,
            "duplicate command wire name {wire_name}"
        );
    }
    for wire_name in queries.iter().map(|entry| entry.wire_name) {
        assert_eq!(
            queries
                .iter()
                .filter(|entry| entry.wire_name == wire_name)
                .count(),
            1,
            "duplicate query wire name {wire_name}"
        );
    }
}

/// 样本表与 registry 登记表必须精确双射：漏加样本（新变体未进穷尽 match）
/// 或多加/改名样本时，两侧 wire 名集合不再相等，本测试失败。
#[test]
fn sample_tables_match_registry_entries_exactly() {
    let sample_commands: std::collections::BTreeSet<&str> =
        command_samples().iter().map(|(name, _)| *name).collect();
    let registry_commands: std::collections::BTreeSet<&str> = command_entries()
        .iter()
        .map(|entry| entry.wire_name)
        .collect();
    assert_eq!(
        sample_commands, registry_commands,
        "command sample table drifted from registry entries"
    );

    let sample_queries: std::collections::BTreeSet<&str> =
        query_samples().iter().map(|(name, _)| *name).collect();
    let registry_queries: std::collections::BTreeSet<&str> = query_entries()
        .iter()
        .map(|entry| entry.wire_name)
        .collect();
    assert_eq!(
        sample_queries, registry_queries,
        "query sample table drifted from registry entries"
    );
}

#[test]
fn unknown_wire_names_fail_closed() {
    assert!(command_by_wire_name("definitely_not_a_command").is_none());
    assert!(query_by_wire_name("definitely_not_a_query").is_none());
    assert!(command_by_wire_name("").is_none());
}

/// GUI 通道宣告向量 golden（V2 快照，2026-08-20 波 A 建立）。
#[test]
fn gui_announcement_vector_matches_v2_snapshot() {
    assert_eq!(
        gui_supported_capabilities(),
        vec![
            GuiCapability::Events,
            GuiCapability::Snapshots,
            GuiCapability::TerminalStreaming,
            GuiCapability::Approvals,
        ]
    );
}

/// K-08 / R0 D13：无任何条目 require ArtifactStreaming，派生宣告不含它。
#[test]
fn no_entry_requires_artifact_streaming() {
    assert!(!command_entries()
        .iter()
        .chain(query_entries().iter())
        .any(|entry| entry.gui.required_capability == Some(GuiCapability::ArtifactStreaming)));
}

fn assert_command_entry(
    command: &AppCommand,
    wire_name: &str,
    gui_available: bool,
    gui_capability: Option<GuiCapability>,
    headless: Option<SdkCapability>,
    acp: bool,
    idempotent: bool,
    since: ApiVersion,
) {
    let entry = command_entry(command);
    assert_eq!(entry.wire_name, wire_name, "wire: {wire_name}");
    assert_eq!(entry.gui.available, gui_available, "gui: {wire_name}");
    assert_eq!(
        entry.gui.required_capability, gui_capability,
        "gui cap: {wire_name}"
    );
    assert_eq!(entry.headless, headless, "headless: {wire_name}");
    assert_eq!(entry.acp, acp, "acp: {wire_name}");
    assert_eq!(entry.idempotent, idempotent, "idempotent: {wire_name}");
    assert_eq!(entry.since, since, "since: {wire_name}");
}

#[test]
fn command_registry_covers_every_variant_without_wildcard() {
    for command in sample_commands() {
        // 无通配符穷尽 match：新增变体必须在此补登记断言。
        match &command {
            AppCommand::CoreInitialize => assert_command_entry(
                &command,
                "core_initialize",
                false,
                None,
                None,
                false,
                true,
                V1_0,
            ),
            AppCommand::WorkspaceAdd { .. } => assert_command_entry(
                &command,
                "workspace_add",
                true,
                None,
                None,
                false,
                false,
                V1_0,
            ),
            AppCommand::WorkspaceTrust { .. } => assert_command_entry(
                &command,
                "workspace_trust",
                false,
                None,
                None,
                false,
                true,
                V1_0,
            ),
            AppCommand::SessionCreate { .. } => assert_command_entry(
                &command,
                "session_create",
                true,
                None,
                Some(SdkCapability::Sessions),
                true,
                false,
                V1_0,
            ),
            AppCommand::SessionOpen { .. } => assert_command_entry(
                &command,
                "session_open",
                true,
                None,
                Some(SdkCapability::Sessions),
                false,
                true,
                V1_0,
            ),
            AppCommand::SessionFork { .. } => assert_command_entry(
                &command,
                "session_fork",
                true,
                None,
                Some(SdkCapability::Sessions),
                false,
                false,
                V1_0,
            ),
            AppCommand::SessionCompact { .. } => assert_command_entry(
                &command,
                "session_compact",
                false,
                None,
                Some(SdkCapability::Sessions),
                false,
                false,
                V1_0,
            ),
            AppCommand::SessionClientContextReplace { .. } => assert_command_entry(
                &command,
                "session_client_context_replace",
                false,
                None,
                Some(SdkCapability::Sessions),
                false,
                true,
                V1_0,
            ),
            AppCommand::RunStart { .. } => assert_command_entry(
                &command,
                "run_start",
                true,
                None,
                Some(SdkCapability::Runs),
                true,
                false,
                V1_2,
            ),
            AppCommand::RunCancel { .. } => assert_command_entry(
                &command,
                "run_cancel",
                true,
                None,
                Some(SdkCapability::Runs),
                true,
                true,
                V1_0,
            ),
            AppCommand::RunRetry { .. } => assert_command_entry(
                &command,
                "run_retry",
                false,
                None,
                Some(SdkCapability::Runs),
                false,
                false,
                V1_0,
            ),
            AppCommand::RunTool { .. } => assert_command_entry(
                &command,
                "run_tool",
                false,
                None,
                Some(SdkCapability::Runs),
                false,
                false,
                V1_0,
            ),
            AppCommand::AuthStart { .. } => {
                assert_command_entry(&command, "auth_start", true, None, None, false, false, V1_4)
            }
            AppCommand::AuthRemove { .. } => {
                assert_command_entry(&command, "auth_remove", true, None, None, false, true, V1_4)
            }
            AppCommand::AuthSetApiKey { .. } => assert_command_entry(
                &command,
                "auth_set_api_key",
                true,
                None,
                None,
                false,
                true,
                V1_4,
            ),
            AppCommand::AuthCancel { .. } => {
                assert_command_entry(&command, "auth_cancel", true, None, None, false, true, V1_4)
            }
            AppCommand::SetDefaultModel { .. } => assert_command_entry(
                &command,
                "set_default_model",
                true,
                None,
                None,
                false,
                true,
                V1_4,
            ),
            AppCommand::ToolApprove { .. } => assert_command_entry(
                &command,
                "tool_approve",
                true,
                Some(GuiCapability::Approvals),
                Some(SdkCapability::Runs),
                true,
                true,
                V1_0,
            ),
            AppCommand::GitStage { .. } => {
                assert_command_entry(&command, "git_stage", false, None, None, false, true, V1_0)
            }
            AppCommand::TerminalCreate { .. } => assert_command_entry(
                &command,
                "terminal_create",
                true,
                Some(GuiCapability::TerminalStreaming),
                None,
                false,
                false,
                V1_0,
            ),
            AppCommand::TerminalWrite { .. } => assert_command_entry(
                &command,
                "terminal_write",
                true,
                Some(GuiCapability::TerminalStreaming),
                None,
                false,
                false,
                V1_0,
            ),
            AppCommand::TerminalResize { .. } => assert_command_entry(
                &command,
                "terminal_resize",
                true,
                Some(GuiCapability::TerminalStreaming),
                None,
                false,
                true,
                V1_0,
            ),
            AppCommand::TerminalClose { .. } => assert_command_entry(
                &command,
                "terminal_close",
                true,
                Some(GuiCapability::TerminalStreaming),
                None,
                false,
                false,
                V1_3,
            ),
        }
    }
}

fn assert_query_entry(
    query: &AppQuery,
    wire_name: &str,
    gui_available: bool,
    gui_capability: Option<GuiCapability>,
    headless: Option<SdkCapability>,
    acp: bool,
    idempotent: bool,
    since: ApiVersion,
) {
    let entry = query_entry(query);
    assert_eq!(entry.wire_name, wire_name, "wire: {wire_name}");
    assert_eq!(entry.gui.available, gui_available, "gui: {wire_name}");
    assert_eq!(
        entry.gui.required_capability, gui_capability,
        "gui cap: {wire_name}"
    );
    assert_eq!(entry.headless, headless, "headless: {wire_name}");
    assert_eq!(entry.acp, acp, "acp: {wire_name}");
    assert_eq!(entry.idempotent, idempotent, "idempotent: {wire_name}");
    assert_eq!(entry.since, since, "since: {wire_name}");
}

#[test]
fn query_registry_covers_every_variant_without_wildcard() {
    for query in sample_queries() {
        // 无通配符穷尽 match：新增变体必须在此补登记断言。
        match &query {
            AppQuery::WorkspaceList => assert_query_entry(
                &query,
                "workspace_list",
                true,
                None,
                None,
                false,
                true,
                V1_0,
            ),
            AppQuery::SessionGet { .. } => assert_query_entry(
                &query,
                "session_get",
                true,
                None,
                Some(SdkCapability::Sessions),
                false,
                true,
                V1_1,
            ),
            AppQuery::RunStatus { .. } => assert_query_entry(
                &query,
                "run_status",
                true,
                None,
                Some(SdkCapability::Runs),
                false,
                true,
                V1_0,
            ),
            AppQuery::ModelList { .. } => {
                assert_query_entry(&query, "model_list", true, None, None, false, true, V1_0)
            }
            AppQuery::DiffListFiles { .. } => assert_query_entry(
                &query,
                "diff_list_files",
                true,
                None,
                None,
                false,
                true,
                V1_0,
            ),
            AppQuery::DiffGet { .. } => {
                assert_query_entry(&query, "diff_get", true, None, None, false, true, V1_0)
            }
            AppQuery::ArtifactRead { .. } => assert_query_entry(
                &query,
                "artifact_read",
                false,
                None,
                None,
                false,
                true,
                V1_0,
            ),
            AppQuery::QuotaOverview { .. } => assert_query_entry(
                &query,
                "quota_overview",
                true,
                None,
                None,
                false,
                true,
                V1_0,
            ),
            AppQuery::SnapshotFetch => assert_query_entry(
                &query,
                "snapshot_fetch",
                false,
                Some(GuiCapability::Snapshots),
                None,
                false,
                true,
                V1_0,
            ),
            AppQuery::PluginList => {
                assert_query_entry(&query, "plugin_list", false, None, None, false, true, V1_0)
            }
            AppQuery::McpList => {
                assert_query_entry(&query, "mcp_list", true, None, None, false, true, V1_0)
            }
            AppQuery::ProviderAuthStatus { .. } => assert_query_entry(
                &query,
                "provider_auth_status",
                true,
                None,
                None,
                false,
                true,
                V1_4,
            ),
        }
    }
}
