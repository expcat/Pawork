//! `headless --json-stdio` 模式：NDJSON 协议接入层（P17-8）。
//!
//! [`HeadlessHandler`] 实现 [`headless_json::stdio::Handler`]：
//!
//! - **握手**：`hello` 帧做版本协商（major 相同取最高共同 minor）与能力
//!   授予；不兼容时返回显式 `IncompatibleApiVersion` 错误帧。
//! - **Command / Query**：复用 [`AppService`] 的信封分发——与 CLI 其他模式
//!   同一 AppService、同一 Event Hub，不引入第二个 Core 宿主。
//! - **事件**：订阅 Event Hub，把全局事件流编码为 `event` 帧写出（EventPump
//!   由调用方运行，本模式不自行轮询）。
//! - **compat 入口**：`compat_import` / `compat_history` 映射到
//!   `session-store` 的持久化实现（P16-10 已收敛存储语义；本层只做协议翻译
//!   与错误映射，不重做存储）。外部内容**只解析不执行**，Secret 由存储层
//!   扫描并拒绝，不复制任何凭据。
//!
//! 与 GUI Connection Protocol 正交：本模式不触碰 `gui-protocol` /
//! `gui-server`，GUI 帧不向本协议泄漏，反之亦然。

use std::sync::Arc;

use agent_domain::SessionId;
use app_service::AppService;
use async_trait::async_trait;
use core_api::{
    ActorIdentity, ApiVersion, AppCommand, AppCommandEnvelope, AppQuery, AppQueryEnvelope,
    AppResponse, CommandSource, API_VERSION,
};
use headless_json::stdio::Handler;
use headless_json::wire::{
    CompatHistoryEntry, CompatHistoryQuery, CompatImportReport, CompatImportRequest, CompatSource,
    HeadlessResponse, HelloRequest, ProtocolErrorKind, SdkCapability, TranslatedRequest,
};
use session_store::{ExternalSource, SessionStore};
use std::collections::BTreeSet;
use subscription_hub::{EventHub, HubError, HubSubscription};

/// Host 支持的全部 SDK 能力（与 GUI Connection Protocol 的 capabilities 正交）。
pub const HOST_CAPABILITIES: &[SdkCapability] = &[
    SdkCapability::Sessions,
    SdkCapability::Runs,
    SdkCapability::Streaming,
    SdkCapability::CompatImport,
    SdkCapability::CompatHistory,
];

/// P17-5 主审修复：headless 协议没有客户端身份概念，宿主的权威来源/身份
/// 固定为 Automation——线上信封携带的 source/identity 一律视为可伪造，
/// 进入 app-service 前强制重写（command 与 query 同理）。
const HEADLESS_IDENTITY_NAME: &str = "headless";

fn host_stamp_command(mut envelope: AppCommandEnvelope) -> AppCommandEnvelope {
    envelope.source = CommandSource::Automation;
    envelope.identity = ActorIdentity::Automation {
        name: HEADLESS_IDENTITY_NAME.into(),
    };
    envelope
}

fn host_stamp_query(mut envelope: AppQueryEnvelope) -> AppQueryEnvelope {
    envelope.source = CommandSource::Automation;
    envelope.identity = ActorIdentity::Automation {
        name: HEADLESS_IDENTITY_NAME.into(),
    };
    envelope
}

/// headless 模式的 Host 接线层：把 NDJSON 请求分发给 AppService / SessionStore。
pub struct HeadlessHandler {
    service: Arc<AppService>,
    instance: String,
    session_store: Option<Arc<SessionStore>>,
    subscription: HubSubscription,
    /// 握手授予的能力子集（HelloAck 的 `granted` 持久保存；每个请求路径
    /// 都按它强制能力门，未授予或空授予的能力不可使用）。
    granted: Vec<SdkCapability>,
    /// 该连接已 create/open 的 core session 集合（P17-9 审查阻塞）。
    /// SDK/headless 协议按连接标识 client（无 client_session），因此用连接级
    /// ownership 阻止 SessionClientContextReplace 跨 session 写入他人上下文；
    /// canonical adapter 路径（ACP）由 ClientAdapterHost 的 authoritative
    /// registry 核验，二者互补、不重复。
    owned_sessions: BTreeSet<SessionId>,
}

impl HeadlessHandler {
    pub fn new(
        service: Arc<AppService>,
        hub: Arc<EventHub>,
        instance: String,
        session_store: Option<Arc<SessionStore>>,
    ) -> Self {
        Self {
            service,
            instance,
            session_store,
            subscription: hub.subscribe(),
            granted: Vec::new(),
            owned_sessions: BTreeSet::new(),
        }
    }

    /// 版本协商：major 相同取客户端声明的最高 minor；无共同 major 返回 `None`。
    fn negotiate_version(requested: &[ApiVersion]) -> Option<ApiVersion> {
        requested
            .iter()
            .filter(|version| version.is_compatible_with(API_VERSION))
            .max_by_key(|version| version.minor)
            .copied()
    }

    fn handshake_response(&self, hello: HelloRequest) -> HeadlessResponse {
        match Self::negotiate_version(&hello.supported_api_versions) {
            Some(negotiated) => {
                let granted = hello
                    .capabilities
                    .iter()
                    .copied()
                    .filter(|capability| HOST_CAPABILITIES.contains(capability))
                    .collect();
                HeadlessResponse::HelloAck {
                    instance_id: self.instance.clone(),
                    negotiated,
                    granted,
                }
            }
            None => HeadlessResponse::Error {
                request_id: None,
                kind: ProtocolErrorKind::IncompatibleApiVersion,
                message: format!(
                    "no common api version: client offered {:?}, host supports {:?}",
                    hello.supported_api_versions,
                    core_api::SUPPORTED_API_VERSIONS
                ),
            },
        }
    }

    /// 能力门：请求需要 `capability`（`None` 表示通用命令/查询，要求至少
    /// 授予任一能力）。未满足时返回显式 `UnsupportedCapability` error 帧；
    /// 通过时返回 `None`。
    fn gate_capability(
        &self,
        capability: Option<SdkCapability>,
        request_id: Option<String>,
    ) -> Option<HeadlessResponse> {
        match capability {
            Some(capability) if self.granted.contains(&capability) => None,
            Some(capability) => Some(HeadlessResponse::Error {
                request_id,
                kind: ProtocolErrorKind::UnsupportedCapability,
                message: format!("capability `{capability:?}` is not granted to this client"),
            }),
            None if !self.granted.is_empty() => None,
            None => Some(HeadlessResponse::Error {
                request_id,
                kind: ProtocolErrorKind::UnsupportedCapability,
                message: "no capability was granted to this client".into(),
            }),
        }
    }

    /// 事件流背压错误：Hub 订阅落后，显式报出 missed 数（不静默丢）。
    fn lagged_error(missed: u64) -> HeadlessResponse {
        HeadlessResponse::Error {
            request_id: None,
            kind: ProtocolErrorKind::Backpressure,
            message: format!("event stream lagged behind by {missed} events"),
        }
    }

    fn compat_error(request_id: String, message: String) -> HeadlessResponse {
        HeadlessResponse::Error {
            request_id: Some(request_id),
            kind: ProtocolErrorKind::CompatRejected,
            message,
        }
    }

    async fn compat_import(&self, request: CompatImportRequest) -> HeadlessResponse {
        let Some(store) = &self.session_store else {
            return HeadlessResponse::Error {
                request_id: Some(request.request_id),
                kind: ProtocolErrorKind::UnsupportedCapability,
                message: "compat import is unavailable: no session store is attached to this host"
                    .into(),
            };
        };
        let source = map_source(request.source);
        // 外部内容只解析不执行；Secret 由存储层扫描并拒绝（不复制凭据）。
        let result = if request.options.dry_run {
            store.import_compat_dry_run(source, &request.content).await
        } else {
            store.import_compat(source, &request.content).await
        };
        match result {
            Ok(report) => HeadlessResponse::CompatImportResult {
                request_id: request.request_id,
                report: map_report(report),
            },
            Err(error) => Self::compat_error(request.request_id, error.to_string()),
        }
    }

    async fn compat_history(&self, query: CompatHistoryQuery) -> HeadlessResponse {
        let Some(store) = &self.session_store else {
            return HeadlessResponse::Error {
                request_id: Some(query.request_id),
                kind: ProtocolErrorKind::UnsupportedCapability,
                message: "compat history is unavailable: no session store is attached to this host"
                    .into(),
            };
        };
        match store
            .compat_import_history(query.limit, query.cursor.as_deref())
            .await
        {
            Ok(page) => HeadlessResponse::CompatHistoryResult {
                request_id: query.request_id,
                entries: page.entries.into_iter().map(map_history_entry).collect(),
                cursor: page.cursor,
            },
            Err(error) => Self::compat_error(query.request_id, error.to_string()),
        }
    }
}

#[async_trait]
impl Handler for HeadlessHandler {
    async fn handshake(&mut self, hello: HelloRequest) -> HeadlessResponse {
        let response = self.handshake_response(hello);
        // 持久保存授予的能力：后续 command/query/compat/events 全部按它
        // 强制能力门；握手失败时清空（空授予不可使用任何入口）。
        self.granted = match &response {
            HeadlessResponse::HelloAck { granted, .. } => granted.clone(),
            _ => Vec::new(),
        };
        response
    }

    async fn handle(&mut self, request: TranslatedRequest) -> Vec<HeadlessResponse> {
        match request {
            TranslatedRequest::Command(envelope) => {
                if let Some(error) = self.gate_capability(
                    command_capability(&envelope.command),
                    Some(envelope.command_id.as_str().to_string()),
                ) {
                    return vec![error];
                }
                // P17-9 审查阻塞：SessionClientContextReplace 必须落在该连接已
                // create/open 的 core session 上，阻止 SDK/headless 通道跨 session
                // 写入他人上下文。GUI 不发送该命令；canonical adapter 路径由
                // ClientAdapterHost registry 核验 client_session→core_session。
                let (context_target, opens_session) = match &envelope.command {
                    AppCommand::SessionClientContextReplace { session_id, .. } => {
                        (Some(session_id.clone()), false)
                    }
                    AppCommand::SessionCreate { .. } | AppCommand::SessionOpen { .. } => {
                        (None, true)
                    }
                    _ => (None, false),
                };
                if let Some(session_id) = context_target {
                    if !self.owned_sessions.contains(&session_id) {
                        return vec![HeadlessResponse::Error {
                            request_id: Some(envelope.command_id.as_str().to_string()),
                            kind: ProtocolErrorKind::CompatRejected,
                            message: "session_client_context_replace targets a session not opened by this connection".into(),
                        }];
                    }
                }
                let response = self.service.dispatch_envelope(host_stamp_command(envelope));
                // 记录该连接拥有的 core session（仅 create/open 成功放行后续 context 写）。
                if opens_session {
                    if let AppResponse::Data(data) = &response.response {
                        if let Some(id) = data.get("session_id").and_then(|value| value.as_str()) {
                            self.owned_sessions.insert(SessionId::from(id));
                        }
                    }
                }
                vec![HeadlessResponse::Response {
                    // P17-5 主审修复：wire 的 source/identity 不可信，宿主
                    // 固定盖戳 Automation 后再派发（query 同理）。
                    envelope: response,
                }]
            }
            TranslatedRequest::Query(envelope) => {
                if let Some(error) = self.gate_capability(
                    query_capability(&envelope.query),
                    Some(envelope.request_id.as_str().to_string()),
                ) {
                    return vec![error];
                }
                vec![HeadlessResponse::Response {
                    envelope: self.service.dispatch_query(host_stamp_query(envelope)),
                }]
            }
            TranslatedRequest::CompatImport(request) => {
                if let Some(error) = self.gate_capability(
                    Some(SdkCapability::CompatImport),
                    Some(request.request_id.clone()),
                ) {
                    return vec![error];
                }
                vec![self.compat_import(request).await]
            }
            TranslatedRequest::CompatHistory(query) => {
                if let Some(error) = self.gate_capability(
                    Some(SdkCapability::CompatHistory),
                    Some(query.request_id.clone()),
                ) {
                    return vec![error];
                }
                vec![self.compat_history(query).await]
            }
        }
    }

    async fn poll_event(&mut self) -> Option<HeadlessResponse> {
        // 能力门：未授予 Streaming 时绝不写 event 帧（客户端从 hello_ack 的
        // `granted` 已知晓）。保持 pending 避免对运行循环造成忙轮询。
        if !self.granted.contains(&SdkCapability::Streaming) {
            std::future::pending().await
        }
        match self.subscription.recv().await {
            Ok(envelope) => Some(HeadlessResponse::Event { envelope }),
            // 慢消费落后：显式 backpressure error 帧，带 missed 数，不静默丢。
            Err(HubError::Lagged { missed }) => Some(Self::lagged_error(missed)),
            // 无事件 / 重放不可用 / 发布端已关闭：按全局序列继续，不向客户端
            // 泄漏 Hub 内部状态；Closed 表示发布端已关闭。
            Err(HubError::Empty | HubError::ReplayUnavailable { .. } | HubError::Closed) => None,
        }
    }
}

/// Command 所属能力域；无专属域的通用命令返回 `None`（要求至少授予任一
/// 能力，见 [`HeadlessHandler::gate_capability`]）。
fn command_capability(command: &AppCommand) -> Option<SdkCapability> {
    match command {
        AppCommand::SessionCreate { .. }
        | AppCommand::SessionOpen { .. }
        | AppCommand::SessionFork { .. }
        | AppCommand::SessionCompact { .. }
        | AppCommand::SessionClientContextReplace { .. } => Some(SdkCapability::Sessions),
        AppCommand::RunStart { .. }
        | AppCommand::RunCancel { .. }
        | AppCommand::RunRetry { .. }
        | AppCommand::RunTool { .. }
        | AppCommand::ToolApprove { .. } => Some(SdkCapability::Runs),
        _ => None,
    }
}

/// Query 所属能力域；无专属域的通用查询返回 `None`。
fn query_capability(query: &AppQuery) -> Option<SdkCapability> {
    match query {
        AppQuery::SessionGet { .. } => Some(SdkCapability::Sessions),
        AppQuery::RunStatus { .. } => Some(SdkCapability::Runs),
        _ => None,
    }
}

fn map_source(source: CompatSource) -> ExternalSource {
    match source {
        CompatSource::Claude => ExternalSource::Claude,
        CompatSource::Codex => ExternalSource::Codex,
        CompatSource::Grok => ExternalSource::Grok,
        CompatSource::Cursor => ExternalSource::Cursor,
    }
}

fn map_source_back(source: ExternalSource) -> CompatSource {
    match source {
        ExternalSource::Claude => CompatSource::Claude,
        ExternalSource::Codex => CompatSource::Codex,
        ExternalSource::Grok => CompatSource::Grok,
        ExternalSource::Cursor => CompatSource::Cursor,
    }
}

fn map_report(report: session_store::CompatImportReport) -> CompatImportReport {
    CompatImportReport {
        source: report.source.map(map_source_back),
        session_id: report.session_id,
        original_id: report.original_id,
        imported_events: report.imported_events,
        imported_messages: report.imported_messages,
        imported_tool_calls: report.imported_tool_calls,
        imported_tool_results: report.imported_tool_results,
        imported_usages: report.imported_usages,
        imported_reviews: report.imported_reviews,
        raw_records: report.raw_records,
        deduplicated: report.deduplicated,
        unknown_fields: report.unknown_fields,
    }
}

fn map_history_entry(entry: session_store::CompatImportHistoryEntry) -> CompatHistoryEntry {
    CompatHistoryEntry {
        session_id: entry.session_id,
        source: map_source_back(entry.source),
        original_id: entry.original_id,
        imported_events: entry.imported_events,
        imported_at_unix_ms: entry.imported_at_unix_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{
        CommandId, ConnectionId, CoreInstanceId, EventId, GuiClientId, QueryId, RunId, SessionId,
        Timestamp, WorkspaceId,
    };
    use core_api::{
        ActorIdentity, ApiHandle, AppCommand, AppCommandEnvelope, AppEvent, AppEventEnvelope,
        AppQuery, AppQueryEnvelope, AppResponse, ClientContextSnapshot, CommandSource, EventSource,
        EventStream, GlobalSequence, RunState, API_VERSION,
    };
    use headless_json::translate::encode_request;
    use headless_json::wire::HeadlessRequest;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    fn hello_line_with(capabilities: Vec<SdkCapability>) -> String {
        encode_request(&HeadlessRequest::Hello {
            client_name: "cli-host-test".into(),
            client_version: "0.0.0".into(),
            supported_api_versions: vec![API_VERSION],
            capabilities,
        })
        .expect("encode hello")
    }

    fn hello_line() -> String {
        hello_line_with(vec![
            SdkCapability::Sessions,
            SdkCapability::Runs,
            SdkCapability::Streaming,
            SdkCapability::CompatImport,
            SdkCapability::CompatHistory,
        ])
    }

    fn command_line(command: AppCommand, id: &str) -> String {
        encode_request(&HeadlessRequest::Command {
            envelope: AppCommandEnvelope {
                api_version: API_VERSION,
                command_id: CommandId::from(id),
                source: CommandSource::Automation,
                identity: ActorIdentity::Automation {
                    name: "cli-host-test".into(),
                },
                expected_revision: None,
                idempotency_key: None,
                issued_at: Timestamp::from_unix_millis(1),
                command,
            },
        })
        .expect("encode command")
    }

    fn query_line(query: AppQuery) -> String {
        encode_request(&HeadlessRequest::Query {
            envelope: AppQueryEnvelope {
                api_version: API_VERSION,
                request_id: QueryId::from("qry-test"),
                source: CommandSource::Automation,
                identity: ActorIdentity::Automation {
                    name: "cli-host-test".into(),
                },
                issued_at: Timestamp::from_unix_millis(1),
                query,
            },
        })
        .expect("encode query")
    }

    fn parse_frames(output: &[u8]) -> Vec<HeadlessResponse> {
        std::str::from_utf8(output)
            .expect("utf8 output")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("frame"))
            .collect()
    }

    /// 构造一条 Hub 事件（Lagged / 事件能力门测试用；全局流 CoreReady）。
    fn hub_envelope(sequence: u64) -> AppEventEnvelope {
        AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: CoreInstanceId::from("headless-test"),
            event_id: EventId::from(format!("evt-{sequence}")),
            global_sequence: GlobalSequence(sequence),
            stream: EventStream::Global,
            stream_sequence: sequence,
            timestamp: Timestamp::from_unix_millis(sequence),
            source: EventSource::Core,
            payload: AppEvent::CoreReady {
                handle: ApiHandle {
                    instance_id: CoreInstanceId::from("headless-test"),
                    api_version: API_VERSION,
                },
            },
        }
    }

    /// 读一行输出（超时内必有帧）。
    async fn read_frame<R: tokio::io::AsyncBufRead + Unpin>(
        reader: &mut R,
        line: &mut String,
    ) -> HeadlessResponse {
        line.clear();
        let read = tokio::time::timeout(Duration::from_secs(5), reader.read_line(line))
            .await
            .expect("read line timeout")
            .expect("read line");
        assert!(read > 0, "EOF while expecting a frame");
        serde_json::from_str(line.trim()).expect("output frame")
    }

    fn plain_host(instance: &str) -> crate::CliHost {
        crate::CliHost::new(std::sync::Arc::new(AppService::new(instance)))
    }

    const CLAUDE_JSON: &str = r#"{
        "conversation_id": "claude-abc",
        "name": "demo chat",
        "chat_messages": [
            {"sender": "human", "text": "hello"},
            {"sender": "assistant", "text": "hi there"}
        ]
    }"#;

    #[tokio::test]
    async fn handshake_negotiates_version_and_grants_capabilities() {
        let host = plain_host("headless-handshake");
        let input = format!("{}\n", hello_line());
        let mut output = Vec::new();
        host.headless_loop(BufReader::new(input.as_bytes()), &mut output)
            .await
            .expect("headless loop");
        let frames = parse_frames(&output);
        assert_eq!(frames.len(), 1, "only hello_ack: {frames:?}");
        match &frames[0] {
            HeadlessResponse::HelloAck {
                instance_id,
                negotiated,
                granted,
            } => {
                assert_eq!(instance_id, "headless-handshake");
                assert_eq!(*negotiated, API_VERSION);
                assert_eq!(
                    granted,
                    &vec![
                        SdkCapability::Sessions,
                        SdkCapability::Runs,
                        SdkCapability::Streaming,
                        SdkCapability::CompatImport,
                        SdkCapability::CompatHistory,
                    ]
                );
            }
            other => panic!("expected hello_ack, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn wire_source_and_identity_are_rewritten_to_automation() {
        // P17-5 主审修复：NDJSON 线上伪造的 source/identity 不进入
        // app-service，宿主固定盖戳 Automation（command 与 query 同理）。
        let host = plain_host("headless-stamp");
        let root = tempfile::tempdir().expect("tempdir");
        let root_path = root.path().display().to_string();
        let forged_command = encode_request(&HeadlessRequest::Command {
            envelope: AppCommandEnvelope {
                api_version: API_VERSION,
                command_id: CommandId::from("cmd-forged"),
                source: CommandSource::LocalGui {
                    client_id: GuiClientId::from("forged"),
                },
                identity: ActorIdentity::System,
                expected_revision: None,
                idempotency_key: None,
                issued_at: Timestamp::from_unix_millis(1),
                command: AppCommand::WorkspaceAdd { root_path },
            },
        })
        .expect("encode forged command");
        let forged_query = encode_request(&HeadlessRequest::Query {
            envelope: AppQueryEnvelope {
                api_version: API_VERSION,
                request_id: QueryId::from("q-forged"),
                source: CommandSource::RemoteGui {
                    client_id: GuiClientId::from("forged"),
                    connection_id: ConnectionId::from("forged"),
                },
                identity: ActorIdentity::Automation {
                    name: "forged".into(),
                },
                issued_at: Timestamp::from_unix_millis(1),
                query: AppQuery::WorkspaceList,
            },
        })
        .expect("encode forged query");
        let input = format!("{}\n{forged_command}\n{forged_query}\n", hello_line());
        let mut output = Vec::new();
        host.headless_loop(BufReader::new(input.as_bytes()), &mut output)
            .await
            .expect("headless loop");
        let frames = parse_frames(&output);
        assert_eq!(frames.len(), 3, "ack + command + query: {frames:?}");

        let sources = host.service().router().source_stats();
        assert_eq!(
            sources.get("automation"),
            Some(&2),
            "command+query 都必须盖戳 Automation: {sources:?}"
        );
        assert!(
            !sources.contains_key("local_gui"),
            "forged LocalGui 不得透传"
        );
        assert!(
            !sources.contains_key("remote_gui"),
            "forged RemoteGui 不得透传"
        );
        let identities = host.service().router().identity_stats();
        assert_eq!(
            identities.get("automation:headless"),
            Some(&2),
            "身份固定 automation:headless: {identities:?}"
        );
        assert!(!identities.contains_key("system"), "forged System 不得透传");
        assert!(
            !identities.contains_key("automation:forged"),
            "forged Automation 身份不得透传"
        );
    }

    #[tokio::test]
    async fn handshake_rejects_incompatible_api_version() {
        let host = plain_host("headless-version");
        let hello = encode_request(&HeadlessRequest::Hello {
            client_name: "old-client".into(),
            client_version: "0.0.0".into(),
            supported_api_versions: vec![core_api::ApiVersion { major: 9, minor: 0 }],
            capabilities: vec![],
        })
        .expect("encode hello");
        let input = format!("{hello}\n");
        let mut output = Vec::new();
        host.headless_loop(BufReader::new(input.as_bytes()), &mut output)
            .await
            .expect("headless loop");
        let frames = parse_frames(&output);
        match &frames[0] {
            HeadlessResponse::Error {
                kind: ProtocolErrorKind::IncompatibleApiVersion,
                ..
            } => {}
            other => panic!("expected incompatible version error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn query_and_command_route_through_app_service() {
        let host = plain_host("headless-routes");
        let root = tempfile::tempdir().expect("tempdir");
        let root_path = root.path().display().to_string();

        // 第一段：hello + query + workspace add（AppService 保持状态）。
        let input = format!(
            "{}\n{}\n{}\n",
            hello_line(),
            query_line(AppQuery::WorkspaceList),
            command_line(AppCommand::WorkspaceAdd { root_path }, "cmd-workspace",),
        );
        let mut output = Vec::new();
        host.headless_loop(BufReader::new(input.as_bytes()), &mut output)
            .await
            .expect("headless loop");
        let frames = parse_frames(&output);
        assert_eq!(frames.len(), 3, "ack + query + workspace: {frames:?}");
        match &frames[1] {
            HeadlessResponse::Response { envelope } => {
                assert!(matches!(envelope.response, AppResponse::Data(_)));
            }
            other => panic!("expected query response, got {other:?}"),
        }
        match &frames[2] {
            HeadlessResponse::Response { envelope } => {
                let workspace_id = match &envelope.response {
                    AppResponse::Data(value) => value
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .expect("workspace id"),
                    other => panic!("expected workspace add data, got {other:?}"),
                };
                assert!(!workspace_id.is_empty());
            }
            other => panic!("expected workspace add response, got {other:?}"),
        }

        // 第二段：hello + session create（使用上一段创建的 workspace id）。
        let workspace_id = match &frames[2] {
            HeadlessResponse::Response { envelope } => match &envelope.response {
                AppResponse::Data(value) => WorkspaceId::from(
                    value
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .expect("workspace id"),
                ),
                other => panic!("unexpected workspace response: {other:?}"),
            },
            other => panic!("expected workspace response, got {other:?}"),
        };
        let input = format!(
            "{}\n{}\n",
            hello_line(),
            command_line(
                AppCommand::SessionCreate {
                    workspace_id,
                    title: Some("headless session".into()),
                },
                "cmd-create",
            )
        );
        let mut output = Vec::new();
        host.headless_loop(BufReader::new(input.as_bytes()), &mut output)
            .await
            .expect("headless loop");
        let frames = parse_frames(&output);
        assert_eq!(frames.len(), 2, "ack + session: {frames:?}");
        match &frames[1] {
            HeadlessResponse::Response { envelope } => {
                let session_id = match &envelope.response {
                    AppResponse::Data(value) => value
                        .get("session_id")
                        .and_then(serde_json::Value::as_str)
                        .expect("session_id"),
                    other => panic!("expected session create data, got {other:?}"),
                };
                assert!(!session_id.is_empty());
            }
            other => panic!("expected command response, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn streams_run_events_from_hub_while_reading_requests() {
        let runtime = core_runtime::CoreRuntime::new("headless-events");
        runtime.register_provider(std::sync::Arc::new(test_support::MockProvider::new(
            test_support::MockScript::new()
                .text("hello from headless")
                .complete(),
        )));
        let host = crate::CliHost::with_hub(
            std::sync::Arc::clone(runtime.service()),
            std::sync::Arc::clone(runtime.hub()),
        );

        // 先经同一 AppService 建会话（RunStart 要求会话存在）。
        let root = tempfile::tempdir().expect("tempdir");
        let root_path = root.path().display().to_string();
        let workspace = host.service().dispatch_envelope(AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from("cmd-workspace"),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation {
                name: "cli-host-test".into(),
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: Timestamp::from_unix_millis(1),
            command: AppCommand::WorkspaceAdd { root_path },
        });
        let workspace_id = match workspace.response {
            AppResponse::Data(value) => WorkspaceId::from(
                value
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .expect("workspace id"),
            ),
            other => panic!("workspace add failed: {other:?}"),
        };
        let created = host.service().dispatch_envelope(AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from("cmd-session"),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation {
                name: "cli-host-test".into(),
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: Timestamp::from_unix_millis(1),
            command: AppCommand::SessionCreate {
                workspace_id,
                title: Some("headless events".into()),
            },
        });
        let session_id = match created.response {
            AppResponse::Data(value) => SessionId::from(
                value
                    .get("session_id")
                    .and_then(serde_json::Value::as_str)
                    .expect("session_id"),
            ),
            other => panic!("session create failed: {other:?}"),
        };

        let (mut input_tx, input_rx) = tokio::io::duplex(1024 * 1024);
        let (output_writer, mut output_rx) = tokio::io::duplex(1024 * 1024);
        let task = tokio::spawn(async move {
            host.headless_loop(BufReader::new(input_rx), output_writer)
                .await
        });

        input_tx
            .write_all(format!("{}\n", hello_line()).as_bytes())
            .await
            .expect("write hello");
        let run_line = command_line(
            AppCommand::RunStart {
                session_id,
                user_message: "hello".into(),
                model: None,
                profile: None,
            },
            "cmd-run",
        );
        input_tx
            .write_all(format!("{run_line}\n").as_bytes())
            .await
            .expect("write run start");

        // 读输出直到同时看到 Response 与 Event 帧（或超时）。
        let mut reader = BufReader::new(&mut output_rx);
        let mut saw_response = false;
        let mut saw_event = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut line = String::new();
        while !(saw_response && saw_event) && tokio::time::Instant::now() < deadline {
            line.clear();
            let read =
                tokio::time::timeout(Duration::from_millis(500), reader.read_line(&mut line))
                    .await
                    .expect("read line")
                    .expect("output line");
            if read == 0 {
                break;
            }
            let frame: HeadlessResponse = serde_json::from_str(line.trim()).expect("output frame");
            match frame {
                HeadlessResponse::HelloAck { .. } => {}
                HeadlessResponse::Response { envelope } => {
                    assert!(matches!(envelope.response, AppResponse::Accepted { .. }));
                    saw_response = true;
                }
                HeadlessResponse::Event { envelope } => {
                    if matches!(
                        envelope.payload,
                        core_api::AppEvent::RunChanged {
                            state: RunState::Completed,
                            ..
                        }
                    ) {
                        saw_event = true;
                    }
                }
                other => panic!("unexpected frame during run: {other:?}"),
            }
        }
        assert!(saw_response, "RunStart response observed");
        assert!(saw_event, "RunChanged(Completed) event observed");

        drop(input_tx); // EOF：运行循环正常结束。
        task.await.expect("headless loop task").expect("loop ok");
    }

    #[tokio::test]
    async fn compat_import_persists_and_history_lists_it() {
        let host = plain_host("headless-compat");
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = SessionStore::open(dir.path().join("session.db"))
            .await
            .expect("open store");
        let mut host = host;
        host.attach_session_store(std::sync::Arc::new(store));

        let import = encode_request(&HeadlessRequest::CompatImport {
            request_id: "ci-1".into(),
            source: CompatSource::Claude,
            content: CLAUDE_JSON.into(),
            options: None,
        })
        .expect("encode import");
        let history = encode_request(&HeadlessRequest::CompatHistory {
            request_id: "ch-1".into(),
            limit: Some(10),
            cursor: None,
        })
        .expect("encode history");
        let input = format!("{}\n{import}\n{history}\n", hello_line());
        let mut output = Vec::new();
        host.headless_loop(BufReader::new(input.as_bytes()), &mut output)
            .await
            .expect("headless loop");
        let frames = parse_frames(&output);
        assert_eq!(frames.len(), 3, "ack + import + history: {frames:?}");
        match &frames[1] {
            HeadlessResponse::CompatImportResult { request_id, report } => {
                assert_eq!(request_id, "ci-1");
                assert_eq!(report.source, Some(CompatSource::Claude));
                assert_eq!(report.imported_messages, 2);
                assert!(!report.session_id.is_empty());
                assert!(!report.deduplicated);
            }
            other => panic!("expected import result, got {other:?}"),
        }
        match &frames[2] {
            HeadlessResponse::CompatHistoryResult {
                request_id,
                entries,
                cursor,
            } => {
                assert_eq!(request_id, "ch-1");
                assert_eq!(entries.len(), 1, "import persisted into history");
                assert_eq!(entries[0].source, CompatSource::Claude);
                assert_eq!(entries[0].original_id.as_deref(), Some("claude-abc"));
                assert!(cursor.is_none());
            }
            other => panic!("expected history result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn compat_dry_run_does_not_persist() {
        let host = plain_host("headless-dry-run");
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = SessionStore::open(dir.path().join("session.db"))
            .await
            .expect("open store");
        let mut host = host;
        host.attach_session_store(std::sync::Arc::new(store));

        let dry = encode_request(&HeadlessRequest::CompatImport {
            request_id: "ci-dry".into(),
            source: CompatSource::Codex,
            content: concat!(
                r#"{"type":"message","role":"user","content":[{"type":"input_text","text":"run tests"}]}"#,
                "\n",
                r#"{"type":"message","role":"assistant","content":[{"type":"output_text","text":"on it"}]}"#,
            )
            .into(),
            options: Some(headless_json::wire::CompatImportOptions { dry_run: true }),
        })
        .expect("encode dry run");
        let history = encode_request(&HeadlessRequest::CompatHistory {
            request_id: "ch-dry".into(),
            limit: None,
            cursor: None,
        })
        .expect("encode history");
        let input = format!("{}\n{dry}\n{history}\n", hello_line());
        let mut output = Vec::new();
        host.headless_loop(BufReader::new(input.as_bytes()), &mut output)
            .await
            .expect("headless loop");
        let frames = parse_frames(&output);
        match &frames[1] {
            HeadlessResponse::CompatImportResult { report, .. } => {
                assert!(report.imported_events > 0, "dry run parses content");
                assert_eq!(report.imported_messages, 2);
            }
            other => panic!("expected dry-run result, got {other:?}"),
        }
        match &frames[2] {
            HeadlessResponse::CompatHistoryResult { entries, .. } => {
                assert!(entries.is_empty(), "dry run must not persist: {entries:?}");
            }
            other => panic!("expected history result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn compat_rejects_secret_and_does_not_persist() {
        let host = plain_host("headless-secret");
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = SessionStore::open(dir.path().join("session.db"))
            .await
            .expect("open store");
        let mut host = host;
        host.attach_session_store(std::sync::Arc::new(store));

        // 命中 sk-ant- 签名（尾随 20 个字符）→ 存储层拒绝，协议返回
        // CompatRejected；外部内容不执行、不落库。
        let malicious = r#"{"conversation_id":"c-secret","chat_messages":[{"sender":"human","text":"key=sk-ant-0123456789abcdefghij"}]}"#.to_string();
        let import = encode_request(&HeadlessRequest::CompatImport {
            request_id: "ci-secret".into(),
            source: CompatSource::Claude,
            content: malicious,
            options: None,
        })
        .expect("encode import");
        let history = encode_request(&HeadlessRequest::CompatHistory {
            request_id: "ch-secret".into(),
            limit: None,
            cursor: None,
        })
        .expect("encode history");
        let input = format!("{}\n{import}\n{history}\n", hello_line());
        let mut output = Vec::new();
        host.headless_loop(BufReader::new(input.as_bytes()), &mut output)
            .await
            .expect("headless loop");
        let frames = parse_frames(&output);
        match &frames[1] {
            HeadlessResponse::Error {
                request_id,
                kind,
                message,
            } => {
                assert_eq!(request_id.as_deref(), Some("ci-secret"));
                assert_eq!(*kind, ProtocolErrorKind::CompatRejected);
                assert!(message.contains("secret"), "message: {message}");
            }
            other => panic!("expected secret rejection, got {other:?}"),
        }
        match &frames[2] {
            HeadlessResponse::CompatHistoryResult { entries, .. } => {
                assert!(entries.is_empty(), "rejected import must not persist");
            }
            other => panic!("expected history result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn compat_without_session_store_returns_unsupported() {
        let host = plain_host("headless-no-store");
        let import = encode_request(&HeadlessRequest::CompatImport {
            request_id: "ci-ns".into(),
            source: CompatSource::Claude,
            content: CLAUDE_JSON.into(),
            options: None,
        })
        .expect("encode import");
        let input = format!("{}\n{import}\n", hello_line());
        let mut output = Vec::new();
        host.headless_loop(BufReader::new(input.as_bytes()), &mut output)
            .await
            .expect("headless loop");
        let frames = parse_frames(&output);
        match &frames[1] {
            HeadlessResponse::Error {
                kind: ProtocolErrorKind::UnsupportedCapability,
                ..
            } => {}
            other => panic!("expected unsupported capability, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn capability_gate_enforces_granted_domains() {
        let host = plain_host("headless-caps");
        let hello = hello_line_with(vec![SdkCapability::Sessions, SdkCapability::Streaming]);
        let import = encode_request(&HeadlessRequest::CompatImport {
            request_id: "ci-x".into(),
            source: CompatSource::Claude,
            content: CLAUDE_JSON.into(),
            options: None,
        })
        .expect("encode import");
        let input = format!(
            "{}\n{}\n{}\n{}\n{}\n{import}\n",
            hello,
            query_line(AppQuery::WorkspaceList),
            command_line(
                AppCommand::SessionCreate {
                    workspace_id: WorkspaceId::from("ws-x"),
                    title: None,
                },
                "cmd-session",
            ),
            command_line(
                AppCommand::RunStart {
                    session_id: SessionId::from("s-x"),
                    user_message: "hi".into(),
                    model: None,
                    profile: None,
                },
                "cmd-run",
            ),
            query_line(AppQuery::RunStatus {
                run_id: RunId::from("r-x"),
            }),
        );
        let mut output = Vec::new();
        host.headless_loop(BufReader::new(input.as_bytes()), &mut output)
            .await
            .expect("headless loop");
        let frames = parse_frames(&output);
        assert_eq!(frames.len(), 6, "ack + 5 requests: {frames:?}");
        // 通用 query / Sessions 命令：门放行（业务结果不在此断言）。
        assert!(matches!(frames[1], HeadlessResponse::Response { .. }));
        assert!(matches!(frames[2], HeadlessResponse::Response { .. }));
        // Runs 未授予：RunStart / RunStatus 显式拒绝，request_id 保留。
        for (index, expected_id) in [(3usize, "cmd-run"), (4, "qry-test")] {
            match &frames[index] {
                HeadlessResponse::Error {
                    request_id,
                    kind,
                    message,
                } => {
                    assert_eq!(request_id.as_deref(), Some(expected_id));
                    assert_eq!(*kind, ProtocolErrorKind::UnsupportedCapability);
                    assert!(message.contains("Runs"), "message: {message}");
                }
                other => panic!("expected capability error at #{index}, got {other:?}"),
            }
        }
        // CompatImport 未授予：显式拒绝。
        match &frames[5] {
            HeadlessResponse::Error {
                request_id,
                kind,
                message,
            } => {
                assert_eq!(request_id.as_deref(), Some("ci-x"));
                assert_eq!(*kind, ProtocolErrorKind::UnsupportedCapability);
                assert!(message.contains("CompatImport"), "message: {message}");
            }
            other => panic!("expected compat capability error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn empty_granted_rejects_all_usage() {
        let host = plain_host("headless-empty-grant");
        let hello = hello_line_with(vec![]);
        let import = encode_request(&HeadlessRequest::CompatImport {
            request_id: "ci-e".into(),
            source: CompatSource::Codex,
            content: CLAUDE_JSON.into(),
            options: None,
        })
        .expect("encode import");
        let input = format!(
            "{}\n{}\n{}\n{import}\n",
            hello,
            query_line(AppQuery::WorkspaceList),
            command_line(
                AppCommand::WorkspaceAdd {
                    root_path: ".".into(),
                },
                "cmd-ws",
            ),
        );
        let mut output = Vec::new();
        host.headless_loop(BufReader::new(input.as_bytes()), &mut output)
            .await
            .expect("headless loop");
        let frames = parse_frames(&output);
        assert_eq!(frames.len(), 4, "ack + 3 rejections: {frames:?}");
        for (index, expected_id) in [(1usize, "qry-test"), (2, "cmd-ws"), (3, "ci-e")] {
            match &frames[index] {
                HeadlessResponse::Error {
                    request_id,
                    kind,
                    message,
                } => {
                    assert_eq!(request_id.as_deref(), Some(expected_id));
                    assert_eq!(*kind, ProtocolErrorKind::UnsupportedCapability);
                    assert!(message.contains("granted"), "message: {message}");
                }
                other => panic!("expected capability error at #{index}, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn events_are_gated_on_streaming_capability() {
        let hub = Arc::new(EventHub::new());
        let host =
            crate::CliHost::with_hub(Arc::new(AppService::new("headless-no-stream")), hub.clone());
        let (mut input_tx, input_rx) = tokio::io::duplex(1024 * 1024);
        let (output_writer, mut output_rx) = tokio::io::duplex(1024 * 1024);
        let task = tokio::spawn(async move {
            host.headless_loop(BufReader::new(input_rx), output_writer)
                .await
        });

        // 只请求 Sessions + Runs（无 Streaming）。
        let hello = hello_line_with(vec![SdkCapability::Sessions, SdkCapability::Runs]);
        input_tx
            .write_all(format!("{hello}\n").as_bytes())
            .await
            .expect("write hello");
        let mut reader = BufReader::new(&mut output_rx);
        let mut line = String::new();
        let ack = read_frame(&mut reader, &mut line).await;
        match ack {
            HeadlessResponse::HelloAck { granted, .. } => {
                assert_eq!(granted, vec![SdkCapability::Sessions, SdkCapability::Runs]);
            }
            other => panic!("expected hello_ack, got {other:?}"),
        }

        // Hub 有事件：未授予 Streaming → 不写任何 event 帧。
        hub.publish(hub_envelope(1));
        hub.publish(hub_envelope(2));
        drop(input_tx); // EOF：运行循环正常结束。
        task.await.expect("headless loop task").expect("loop ok");
        drop(reader);
        let mut output = Vec::new();
        output_rx
            .read_to_end(&mut output)
            .await
            .expect("read remaining output");
        let frames = parse_frames(&output);
        assert!(
            frames.is_empty(),
            "no event frames without Streaming grant: {frames:?}"
        );
    }

    #[tokio::test]
    async fn hub_lagged_emits_explicit_backpressure_error_with_missed_count() {
        let hub = Arc::new(EventHub::with_capacity(2));
        let host =
            crate::CliHost::with_hub(Arc::new(AppService::new("headless-lagged")), hub.clone());
        let (mut input_tx, input_rx) = tokio::io::duplex(1024 * 1024);
        let (output_writer, mut output_rx) = tokio::io::duplex(1024 * 1024);
        let task = tokio::spawn(async move {
            host.headless_loop(BufReader::new(input_rx), output_writer)
                .await
        });

        input_tx
            .write_all(format!("{}\n", hello_line()).as_bytes())
            .await
            .expect("write hello");
        let mut reader = BufReader::new(&mut output_rx);
        let mut line = String::new();
        let ack = read_frame(&mut reader, &mut line).await;
        assert!(matches!(ack, HeadlessResponse::HelloAck { .. }));

        // 容量 2 的 Hub 上同步发布 5 条（无 await，运行循环不可能插入消费）
        // → 订阅落后，recv 返回 Lagged(3)。
        for sequence in 1..=5u64 {
            hub.publish(hub_envelope(sequence));
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut saw_backpressure = false;
        let mut saw_event = false;
        while !(saw_backpressure && saw_event) && tokio::time::Instant::now() < deadline {
            line.clear();
            let read =
                tokio::time::timeout(Duration::from_millis(500), reader.read_line(&mut line))
                    .await
                    .expect("read line")
                    .expect("output line");
            if read == 0 {
                break;
            }
            let frame: HeadlessResponse = serde_json::from_str(line.trim()).expect("output frame");
            match frame {
                HeadlessResponse::Error {
                    kind,
                    message,
                    request_id,
                } => {
                    assert_eq!(
                        kind,
                        ProtocolErrorKind::Backpressure,
                        "lag must surface as explicit backpressure error: {message}"
                    );
                    assert!(request_id.is_none());
                    assert!(
                        message.contains("3"),
                        "missed count must be explicit: {message}"
                    );
                    saw_backpressure = true;
                }
                HeadlessResponse::Event { .. } => saw_event = true,
                HeadlessResponse::HelloAck { .. } => {}
                other => panic!("unexpected frame during lag: {other:?}"),
            }
        }
        assert!(saw_backpressure, "explicit backpressure error observed");
        assert!(saw_event, "events resume after the lag error");

        drop(input_tx); // EOF：运行循环正常结束。
        task.await.expect("headless loop task").expect("loop ok");
    }

    #[tokio::test]
    async fn session_client_context_replace_rejects_cross_session_writes() {
        // P17-9 审查阻塞：SDK/headless 通道按连接标识 client，
        // SessionClientContextReplace 只能落在该连接已 create/open 的 session。
        // 分两段：第一段创建 workspace/session，从真实 SessionCreate Data
        // 响应提取 session_id（全局 ID 计数器同时服务 workspace/session，
        // 不能硬编码）；第二段 open 该 session 重建连接级 ownership，
        // 再断言 own write 放行、cross write 被拒绝。
        let host = plain_host("headless-ctx-ownership");
        let root = tempfile::tempdir().expect("tempdir");
        let root_path = root.path().display().to_string();
        let snapshot = ClientContextSnapshot {
            revision: 1,
            active_document: None,
            open_documents: Vec::new(),
            diagnostics: Vec::new(),
        };
        // 第一段：hello → workspace add → session create。
        let input = format!(
            "{}\n{}\n{}\n",
            hello_line(),
            command_line(AppCommand::WorkspaceAdd { root_path }, "cmd-ws"),
            command_line(
                AppCommand::SessionCreate {
                    workspace_id: WorkspaceId::from("workspace-1"),
                    title: None,
                },
                "cmd-sess",
            ),
        );
        let mut output = Vec::new();
        host.headless_loop(BufReader::new(input.as_bytes()), &mut output)
            .await
            .expect("headless loop");
        let frames = parse_frames(&output);
        // hello_ack + workspace + session_create = 3。
        assert_eq!(frames.len(), 3, "frames: {frames:?}");
        // 从真实 SessionCreate Data 响应提取 session_id，不做任何 ID 假设。
        let session_id = match &frames[2] {
            HeadlessResponse::Response { envelope } => match &envelope.response {
                AppResponse::Data(data) => data
                    .get("session_id")
                    .and_then(|value| value.as_str())
                    .map(SessionId::from)
                    .expect("session create Data must carry session_id"),
                other => panic!("session create must return Data, got {other:?}"),
            },
            other => panic!("session create must return a Response frame, got {other:?}"),
        };
        // cross 目标使用明确不同于真实 session 的 ID。
        let cross_id = SessionId::from(format!("{session_id}-other"));
        // 第二段：hello → open 真实 session → own context replace → cross context replace。
        let input = format!(
            "{}\n{}\n{}\n{}\n",
            hello_line(),
            command_line(
                AppCommand::SessionOpen {
                    session_id: session_id.clone(),
                },
                "cmd-open",
            ),
            command_line(
                AppCommand::SessionClientContextReplace {
                    session_id: session_id.clone(),
                    snapshot: snapshot.clone(),
                },
                "cmd-ctx-own",
            ),
            command_line(
                AppCommand::SessionClientContextReplace {
                    session_id: cross_id,
                    snapshot,
                },
                "cmd-ctx-cross",
            ),
        );
        let mut output = Vec::new();
        host.headless_loop(BufReader::new(input.as_bytes()), &mut output)
            .await
            .expect("headless loop");
        let frames = parse_frames(&output);
        // hello_ack + session_open + own_ok + cross_rejected = 4。
        assert_eq!(frames.len(), 4, "frames: {frames:?}");
        // open 建立连接级 ownership（Data 响应）。
        assert!(
            matches!(&frames[1], HeadlessResponse::Response { envelope } if matches!(envelope.response, AppResponse::Data(_))),
            "session open must be allowed: {:?}",
            frames[1]
        );
        // own-session 写放行（Data 响应）。
        assert!(
            matches!(&frames[2], HeadlessResponse::Response { envelope } if matches!(envelope.response, AppResponse::Data(_))),
            "own-session context replace must be allowed: {:?}",
            frames[2]
        );
        // cross-session 写被连接级 ownership 拒绝，aggregate 不被触达。
        match &frames[3] {
            HeadlessResponse::Error {
                kind: ProtocolErrorKind::CompatRejected,
                ..
            } => {}
            other => panic!("expected cross-session rejection, got {other:?}"),
        }
    }
}
