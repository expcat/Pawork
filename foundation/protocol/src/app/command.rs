//! 应用层命令信封、身份与客户端上下文。

use std::{fmt, path::Component, str::FromStr};

use pawork_domain::{
    ActorId, CommandId, ConnectionId, EventId, GuiClientId, ModelId, PluginId, ProviderId, RunId,
    SessionId, Timestamp, ToolCallId, WorkspaceId,
};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use thiserror::Error;
#[cfg(feature = "typegen")]
use ts_rs::TS;

use super::version::{ApiVersion, DEFAULT_CONTROL_PLANE_PRINCIPAL};

/// IDE/Host 上下文快照的资源上限。该数据来自外部客户端，Core 必须在存储和
/// 注入模型请求前 fail-closed，避免诊断风暴或超长 URI/消息放大内存与 prompt。
pub const MAX_CLIENT_CONTEXT_BYTES: usize = 1024 * 1024;
pub const MAX_CLIENT_CONTEXT_DOCUMENTS: usize = 128;
pub const MAX_CLIENT_CONTEXT_DIAGNOSTICS: usize = 1024;
pub const MAX_CLIENT_CONTEXT_URI_BYTES: usize = 4096;
pub const MAX_CLIENT_CONTEXT_MESSAGE_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct AppCommandEnvelope {
    pub api_version: ApiVersion,
    pub command_id: CommandId,
    pub source: CommandSource,
    pub identity: ActorIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    pub issued_at: Timestamp,
    pub command: AppCommand,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandSource {
    LocalCli {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminal_session_id: Option<String>,
    },
    LocalGui {
        client_id: GuiClientId,
    },
    RemoteGui {
        client_id: GuiClientId,
        connection_id: ConnectionId,
    },
    Automation,
    Plugin,
    Mcp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActorIdentity {
    LocalUser {
        actor_id: ActorId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_name: Option<String>,
    },
    AuthenticatedClient {
        actor_id: ActorId,
        subject: String,
    },
    Automation {
        name: String,
    },
    Plugin {
        plugin_id: PluginId,
    },
    McpServer {
        server_id: String,
    },
    System,
}

/// Host 观察到的文本位置；采用 LSP 的 zero-based line/character 语义，但不
/// 依赖任何 IDE/LSP crate，保持 Core canonical domain 中立。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct ClientTextPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct ClientTextRange {
    pub start: ClientTextPosition,
    pub end: ClientTextPosition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum ClientDiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

/// 单个打开文档的有限元数据。刻意不携带正文，只保留上下文定位和字节数提示，
/// 避免 IDE 通道变成绕过 Workspace/Policy 的文件读取入口。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct ClientDocumentContext {
    pub uri: String,
    pub language_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<ClientTextRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_range: Option<ClientTextRange>,
    pub saved_version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_bytes: Option<u64>,
}

/// IDE/LSP 展示的诊断快照。`message` 是不可信观察数据，不具备指令权限。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct ClientDiagnostic {
    pub document_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    pub range: ClientTextRange,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<ClientDiagnosticSeverity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub message: String,
}

/// 外部 Host 对一个 Core session 的全量、单调版本化上下文快照。
///
/// 替换语义使断线重连可直接重放最新状态，不需要累积不可恢复的增量日志。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct ClientContextSnapshot {
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_document: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_documents: Vec<ClientDocumentContext>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<ClientDiagnostic>,
}

impl ClientContextSnapshot {
    /// 在 canonical 边界执行有界校验。错误文本只描述字段与预算，不回显外部
    /// 内容，避免诊断消息或 URI 泄漏到日志/协议错误。
    pub fn validate(&self) -> Result<(), String> {
        if self.revision == 0 {
            return Err("revision must be greater than zero".into());
        }
        if self.open_documents.len() > MAX_CLIENT_CONTEXT_DOCUMENTS {
            return Err(format!(
                "open document count exceeds {MAX_CLIENT_CONTEXT_DOCUMENTS}"
            ));
        }
        if self.diagnostics.len() > MAX_CLIENT_CONTEXT_DIAGNOSTICS {
            return Err(format!(
                "diagnostic count exceeds {MAX_CLIENT_CONTEXT_DIAGNOSTICS}"
            ));
        }
        for document in &self.open_documents {
            validate_client_uri(&document.uri)?;
            if document.language_id.is_empty() || document.language_id.len() > 128 {
                return Err("language_id must contain 1..=128 bytes".into());
            }
            validate_client_range(document.selection)?;
            validate_client_range(document.visible_range)?;
        }
        if let Some(active) = self.active_document.as_deref() {
            validate_client_uri(active)?;
            if !self
                .open_documents
                .iter()
                .any(|document| document.uri == active)
            {
                return Err("active_document must name an open document".into());
            }
        }
        for diagnostic in &self.diagnostics {
            validate_client_uri(&diagnostic.document_uri)?;
            validate_client_range(Some(diagnostic.range))?;
            if diagnostic.message.len() > MAX_CLIENT_CONTEXT_MESSAGE_BYTES {
                return Err(format!(
                    "diagnostic message exceeds {MAX_CLIENT_CONTEXT_MESSAGE_BYTES} bytes"
                ));
            }
            for (name, value) in [
                ("diagnostic code", diagnostic.code.as_deref()),
                ("diagnostic source", diagnostic.source.as_deref()),
            ] {
                if value.is_some_and(|value| value.len() > 256) {
                    return Err(format!("{name} exceeds 256 bytes"));
                }
            }
        }
        let encoded = serde_json::to_vec(self)
            .map_err(|_| "client context could not be encoded".to_string())?;
        if encoded.len() > MAX_CLIENT_CONTEXT_BYTES {
            return Err(format!(
                "client context exceeds {MAX_CLIENT_CONTEXT_BYTES} bytes"
            ));
        }
        Ok(())
    }
}

fn validate_client_uri(uri: &str) -> Result<(), String> {
    if uri.is_empty() || uri.len() > MAX_CLIENT_CONTEXT_URI_BYTES {
        return Err(format!(
            "document URI must contain 1..={MAX_CLIENT_CONTEXT_URI_BYTES} bytes"
        ));
    }
    // P17-9 审查阻塞：低信任 URI 必须携带安全 scheme——禁止无 scheme、
    // 畸形 scheme 或可执行脚本 scheme（javascript/data/vbscript），避免
    // observation 通道里的 URI 被误解为可执行/可加载内容。
    let scheme = uri.split(':').next().unwrap_or("");
    let valid_scheme = scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !valid_scheme {
        return Err("document URI must begin with a valid scheme".into());
    }
    if matches!(
        scheme.to_ascii_lowercase().as_str(),
        "javascript" | "data" | "vbscript"
    ) {
        return Err("document URI scheme is not allowed".into());
    }
    Ok(())
}

fn validate_client_range(range: Option<ClientTextRange>) -> Result<(), String> {
    if let Some(range) = range {
        let start = (range.start.line, range.start.character);
        let end = (range.end.line, range.end.character);
        if start > end {
            return Err("text range start must not follow end".into());
        }
    }
    Ok(())
}

impl ActorIdentity {
    /// 映射为 canonical 主体键（P18-2 身份传播）。
    ///
    /// 供 `app-service` 的身份解析器（`tenant-service::IdentityResolver`）消费：
    /// 本地用户 / 已认证客户端 / 自动化 / 插件 / MCP 服务器均能映射出非空主体键；
    /// `System` 显式映射为 `local/system`。任何携带空白 payload 的身份返回
    /// `None`，解析层据此 fail-closed 拒绝，而不是静默落入默认身份。
    pub fn canonical_principal(&self) -> Option<String> {
        match self {
            ActorIdentity::LocalUser { actor_id, .. } if !actor_id.as_str().trim().is_empty() => {
                Some(DEFAULT_CONTROL_PLANE_PRINCIPAL.to_string())
            }
            ActorIdentity::AuthenticatedClient { subject, .. } if !subject.trim().is_empty() => {
                Some(format!("authenticated_client:{}", subject.trim()))
            }
            ActorIdentity::Automation { name } if !name.trim().is_empty() => {
                Some(format!("automation:{}", name.trim()))
            }
            ActorIdentity::Plugin { plugin_id } if !plugin_id.as_str().trim().is_empty() => {
                Some(format!("plugin:{}", plugin_id.as_str().trim()))
            }
            ActorIdentity::McpServer { server_id } if !server_id.trim().is_empty() => {
                Some(format!("mcp_server:{}", server_id.trim()))
            }
            ActorIdentity::System => Some("local/system".to_string()),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum AppCommand {
    CoreInitialize,
    WorkspaceAdd {
        root_path: String,
    },
    WorkspaceTrust {
        workspace_id: WorkspaceId,
        trusted: bool,
    },
    SessionCreate {
        workspace_id: WorkspaceId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    SessionOpen {
        session_id: SessionId,
    },
    SessionFork {
        session_id: SessionId,
        parent_event_id: EventId,
    },
    SessionCompact {
        session_id: SessionId,
    },
    /// Host（IDE/ACP 等）观察到的 session 上下文全量替换。内容是有界的
    /// 不可信数据；Core 仅将它作为 Agent observation，不授予工具或写权限。
    SessionClientContextReplace {
        session_id: SessionId,
        snapshot: ClientContextSnapshot,
    },
    RunStart {
        session_id: SessionId,
        user_message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<ModelId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<ProviderId>,
        /// P17-5：可选 Agent Profile v2 名称。命中生产 `ResourceBundle.profiles_v2`
        /// 时其不可变配置（prompt / canonical effort / tools / max_turns /
        /// background / isolation / memory）成为该 run 的权威来源；未知 /
        /// 跨 workspace / 引用不可用为结构化 fail-closed RunStart 错误。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<String>,
    },
    RunCancel {
        run_id: RunId,
    },
    RunRetry {
        run_id: RunId,
    },
    RunTool {
        run_id: RunId,
        tool_name: String,
        input: Value,
    },
    AuthStart {
        provider_id: ProviderId,
        flow: String,
    },
    AuthRemove {
        provider_id: ProviderId,
    },
    ToolApprove {
        run_id: RunId,
        tool_call_id: ToolCallId,
        decision: ApprovalDecision,
    },
    GitStage {
        workspace_id: WorkspaceId,
        paths: Vec<WorkspaceRelativePath>,
    },
    TerminalCreate {
        workspace_id: WorkspaceId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        working_directory: Option<WorkspaceRelativePath>,
    },
    TerminalWrite {
        terminal_session_id: String,
        data: String,
    },
    TerminalResize {
        terminal_session_id: String,
        columns: u16,
        rows: u16,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    ApproveOnce,
    ApproveForRun,
    Deny,
    Cancel,
}

/// 已验证的 Workspace 相对路径。反序列化同样执行校验，不能绕过构造器。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct WorkspaceRelativePath(String);

impl WorkspaceRelativePath {
    pub fn new(value: impl Into<String>) -> Result<Self, RelativePathError> {
        let value = value.into();
        let bytes = value.as_bytes();
        let has_windows_prefix =
            bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
        let has_cross_platform_parent = value.split(['/', '\\']).any(|component| component == "..");
        if value.is_empty()
            || value.contains('\0')
            || value.starts_with(['/', '\\'])
            || has_windows_prefix
            || has_cross_platform_parent
        {
            return Err(RelativePathError);
        }
        let path = std::path::Path::new(&value);
        if path.is_absolute()
            || path.components().any(|part| {
                matches!(
                    part,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(RelativePathError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkspaceRelativePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for WorkspaceRelativePath {
    type Err = RelativePathError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl Serialize for WorkspaceRelativePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for WorkspaceRelativePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(|_| de::Error::custom("expected a safe workspace-relative path"))
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error("path must be non-empty, workspace-relative, and contain no parent traversal")]
pub struct RelativePathError;

#[cfg(test)]
mod tests {
    use pawork_domain::{
        ActorId, CommandId, ConnectionId, GuiClientId, PluginId, Timestamp, WorkspaceId,
    };

    use super::*;
    use crate::app::API_VERSION;

    fn command_source() -> CommandSource {
        CommandSource::RemoteGui {
            client_id: GuiClientId::from("gui-1"),
            connection_id: ConnectionId::from("connection-1"),
        }
    }

    #[test]
    fn command_envelope_round_trip_preserves_source_identity_and_idempotency() {
        let envelope = AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from("command-1"),
            source: command_source(),
            identity: ActorIdentity::AuthenticatedClient {
                actor_id: ActorId::from("actor-1"),
                subject: "user@example".into(),
            },
            expected_revision: Some(7),
            idempotency_key: Some("create-run-once".into()),
            issued_at: Timestamp::from_unix_millis(1),
            command: AppCommand::GitStage {
                workspace_id: WorkspaceId::from("workspace-1"),
                paths: vec![WorkspaceRelativePath::new("src/lib.rs").expect("relative path")],
            },
        };

        let json = serde_json::to_string(&envelope).expect("serialize command");
        let decoded: AppCommandEnvelope = serde_json::from_str(&json).expect("deserialize command");
        assert_eq!(decoded, envelope);
    }

    fn client_snapshot(revision: u64) -> ClientContextSnapshot {
        ClientContextSnapshot {
            revision,
            active_document: Some("file:///workspace/src/lib.rs".into()),
            open_documents: vec![ClientDocumentContext {
                uri: "file:///workspace/src/lib.rs".into(),
                language_id: "rust".into(),
                selection: Some(ClientTextRange {
                    start: ClientTextPosition {
                        line: 1,
                        character: 2,
                    },
                    end: ClientTextPosition {
                        line: 1,
                        character: 4,
                    },
                }),
                visible_range: None,
                saved_version: 3,
                text_bytes: Some(128),
            }],
            diagnostics: vec![ClientDiagnostic {
                document_uri: "file:///workspace/src/lib.rs".into(),
                version: Some(3),
                range: ClientTextRange {
                    start: ClientTextPosition {
                        line: 1,
                        character: 2,
                    },
                    end: ClientTextPosition {
                        line: 1,
                        character: 4,
                    },
                },
                severity: Some(ClientDiagnosticSeverity::Warning),
                code: Some("unused".into()),
                source: Some("rust-analyzer".into()),
                message: "unused variable".into(),
            }],
        }
    }

    #[test]
    fn client_context_round_trips_and_excludes_document_text() {
        let snapshot = client_snapshot(1);
        snapshot.validate().expect("valid bounded snapshot");
        let json = serde_json::to_string(&snapshot).expect("serialize");
        assert!(!json.contains("fn main"));
        assert_eq!(
            serde_json::from_str::<ClientContextSnapshot>(&json).expect("deserialize"),
            snapshot
        );
    }

    #[test]
    fn client_context_rejects_invalid_ranges_and_resource_overflow() {
        let mut snapshot = client_snapshot(1);
        snapshot.diagnostics[0].range.start.line = 2;
        assert!(snapshot.validate().unwrap_err().contains("range start"));

        let mut snapshot = client_snapshot(1);
        snapshot.diagnostics[0].message = "x".repeat(MAX_CLIENT_CONTEXT_MESSAGE_BYTES + 1);
        assert!(snapshot
            .validate()
            .unwrap_err()
            .contains("diagnostic message"));
    }

    #[test]
    fn client_context_rejects_unsafe_uri_schemes() {
        // P17-9：低信任 URI 必须携带安全 scheme；可执行脚本 scheme 与无 scheme 一律拒绝。
        let mut snapshot = client_snapshot(1);
        snapshot.diagnostics[0].document_uri = "javascript:alert(1)".into();
        assert!(snapshot
            .validate()
            .unwrap_err()
            .contains("scheme is not allowed"));

        let mut snapshot = client_snapshot(1);
        snapshot.diagnostics[0].document_uri = "data:text/html,<script>".into();
        assert!(snapshot
            .validate()
            .unwrap_err()
            .contains("scheme is not allowed"));

        let mut snapshot = client_snapshot(1);
        snapshot.open_documents[0].uri = "1noscheme".into();
        assert!(snapshot.validate().unwrap_err().contains("valid scheme"));

        // 安全 scheme（file/http/untitled/vscode-userdata）放行。
        let mut snapshot = client_snapshot(1);
        snapshot.open_documents[0].uri = "untitled:Untitled-1".into();
        snapshot.active_document = Some("untitled:Untitled-1".into());
        snapshot.diagnostics[0].document_uri = "untitled:Untitled-1".into();
        snapshot.validate().expect("untitled scheme is allowed");
    }

    #[test]
    fn unsafe_paths_are_rejected_even_during_deserialization() {
        assert!(WorkspaceRelativePath::new("../secret").is_err());
        assert!(WorkspaceRelativePath::new("/absolute").is_err());
        assert!(WorkspaceRelativePath::new(r"..\secret").is_err());
        assert!(WorkspaceRelativePath::new(r"C:\Windows").is_err());
        assert!(WorkspaceRelativePath::new(r"C:drive-relative").is_err());
        assert!(WorkspaceRelativePath::new(r"\\server\share").is_err());
        assert!(serde_json::from_str::<WorkspaceRelativePath>(r#""../secret""#).is_err());
        assert!(serde_json::from_str::<WorkspaceRelativePath>(r#""C:\\Windows""#).is_err());
    }

    #[test]
    fn actor_identity_canonical_principal_maps_stable_principals() {
        let cases = [
            (
                ActorIdentity::LocalUser {
                    actor_id: ActorId::from("actor-1"),
                    display_name: None,
                },
                Some("local/user"),
            ),
            (
                ActorIdentity::AuthenticatedClient {
                    actor_id: ActorId::from("actor-2"),
                    subject: "subject-1".into(),
                },
                Some("authenticated_client:subject-1"),
            ),
            (
                ActorIdentity::Automation {
                    name: "scheduler".into(),
                },
                Some("automation:scheduler"),
            ),
            (
                ActorIdentity::Plugin {
                    plugin_id: PluginId::from("plugin-1"),
                },
                Some("plugin:plugin-1"),
            ),
            (
                ActorIdentity::McpServer {
                    server_id: "server-1".into(),
                },
                Some("mcp_server:server-1"),
            ),
        ];
        for (identity, expected) in cases {
            assert_eq!(identity.canonical_principal().as_deref(), expected);
        }
        assert_eq!(
            ActorIdentity::System.canonical_principal().as_deref(),
            Some("local/system")
        );
        assert_eq!(
            ActorIdentity::AuthenticatedClient {
                actor_id: ActorId::from("actor"),
                subject: "   ".into(),
            }
            .canonical_principal(),
            None
        );
        assert_eq!(
            ActorIdentity::Automation { name: "\t".into() }.canonical_principal(),
            None
        );
    }

}
