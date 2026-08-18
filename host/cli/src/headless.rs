//! `pawork headless --json-stdio`：NDJSON 协议入口。

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use pawork_app::{AppCore, GuiHostAdapter};
use pawork_gui_server::GuiHost;
use pawork_domain::SessionId;
use pawork_protocol::headless::stdio::{self, Handler, LoopConfig};
use pawork_protocol::headless::{
    CompatHistoryEntry, CompatImportReport, CompatSource, HeadlessResponse, HelloRequest,
    ProtocolErrorKind, SdkCapability, TranslatedRequest,
};
use pawork_protocol::{
    negotiate_api_version_with, AppCommand, AppQuery, AppResponse, SUPPORTED_API_VERSIONS,
};
use pawork_storage::session::{ExternalSource, SessionStore};
use tokio::io::{stdin, stdout, BufReader};
use tokio::sync::broadcast::error::TryRecvError;

use crate::adapter::{stamp_automation, stamp_query, wrap_response};
use crate::CliError;

const HEADLESS_NAME: &str = "headless";

pub const HOST_CAPABILITIES: &[SdkCapability] = &[
    SdkCapability::Sessions,
    SdkCapability::Runs,
    SdkCapability::Streaming,
    SdkCapability::CompatImport,
    SdkCapability::CompatHistory,
];

pub async fn run_headless(core: AppCore, json_stdio: bool) -> Result<(), CliError> {
    if !json_stdio {
        return Err(CliError::Usage(
            "headless 需要 --json-stdio（stdout 只写 JSONL 协议帧）".into(),
        ));
    }
    let adapter = Arc::new(crate::adapter::adapter_with_gui_approvals(core));
    let mut handler = HeadlessHandler::new(adapter);
    let stdin = BufReader::new(stdin());
    stdio::run_loop(stdin, stdout(), LoopConfig::default(), &mut handler)
        .await
        .map_err(CliError::Io)
}

struct HeadlessHandler {
    adapter: Arc<GuiHostAdapter>,
    events: tokio::sync::broadcast::Receiver<pawork_protocol::AppEventEnvelope>,
    granted: Vec<SdkCapability>,
    owned_sessions: BTreeSet<SessionId>,
}

impl HeadlessHandler {
    fn new(adapter: Arc<GuiHostAdapter>) -> Self {
        let events = adapter.subscribe_events();
        Self {
            adapter,
            events,
            granted: Vec::new(),
            owned_sessions: BTreeSet::new(),
        }
    }

    fn gate(
        &self,
        capability: Option<SdkCapability>,
        request_id: Option<String>,
    ) -> Option<HeadlessResponse> {
        gate_capability(&self.granted, capability, request_id)
    }

    async fn store(&self) -> Result<SessionStore, String> {
        self.adapter
            .session_store()
            .await
            .map_err(|error| error.to_string())
    }
}

#[async_trait]
impl Handler for HeadlessHandler {
    async fn handshake(&mut self, hello: HelloRequest) -> HeadlessResponse {
        let response = match negotiate_api_version_with(
            &hello.supported_api_versions,
            SUPPORTED_API_VERSIONS,
        ) {
            Some(negotiated) => {
                let granted = hello
                    .capabilities
                    .iter()
                    .copied()
                    .filter(|capability| HOST_CAPABILITIES.contains(capability))
                    .collect();
                HeadlessResponse::HelloAck {
                    instance_id: self.adapter.instance_id().as_str().to_string(),
                    negotiated,
                    granted,
                }
            }
            None => HeadlessResponse::Error {
                request_id: None,
                kind: ProtocolErrorKind::IncompatibleApiVersion,
                message: format!(
                    "no common api version: client offered {:?}, host supports {:?}",
                    hello.supported_api_versions, SUPPORTED_API_VERSIONS
                ),
            },
        };
        self.granted = match &response {
            HeadlessResponse::HelloAck { granted, .. } => granted.clone(),
            _ => Vec::new(),
        };
        response
    }

    async fn handle(&mut self, request: TranslatedRequest) -> Vec<HeadlessResponse> {
        match request {
            TranslatedRequest::Command(envelope) => {
                if let Some(error) = self.gate(
                    command_capability(&envelope.command),
                    Some(envelope.command_id.as_str().to_string()),
                ) {
                    return vec![error];
                }
                if let AppCommand::SessionClientContextReplace { session_id, .. } = &envelope.command
                {
                    if !self.owned_sessions.contains(session_id) {
                        return vec![HeadlessResponse::Error {
                            request_id: Some(envelope.command_id.as_str().to_string()),
                            kind: ProtocolErrorKind::CompatRejected,
                            message: "session_client_context_replace targets a session not opened by this connection".into(),
                        }];
                    }
                }
                let opens = matches!(
                    envelope.command,
                    AppCommand::SessionCreate { .. } | AppCommand::SessionOpen { .. }
                );
                let stamped = stamp_automation(envelope, HEADLESS_NAME);
                let request_id = stamped.command_id.as_str().to_string();
                let response = match self.adapter.command(&stamped).await {
                    Ok(response) => response,
                    Err(error) => AppResponse::Error(pawork_domain::ErrorContext {
                        category: pawork_domain::ErrorCategory::Internal,
                        message: error.to_string(),
                        retryable: error.retryable,
                        retry_after_ms: None,
                        diagnostics: Default::default(),
                    }),
                };
                if opens {
                    if let AppResponse::Data(data) = &response {
                        if let Some(id) = data.get("session_id").and_then(|value| value.as_str()) {
                            self.owned_sessions.insert(SessionId::from(id));
                        }
                    }
                }
                vec![HeadlessResponse::Response {
                    envelope: wrap_response(&request_id, response),
                }]
            }
            TranslatedRequest::Query(envelope) => {
                if let Some(error) = self.gate(
                    query_capability(&envelope.query),
                    Some(envelope.request_id.as_str().to_string()),
                ) {
                    return vec![error];
                }
                let stamped = stamp_query(envelope, HEADLESS_NAME);
                let request_id = stamped.request_id.as_str().to_string();
                let response = match self.adapter.query(&stamped).await {
                    Ok(response) => response,
                    Err(error) => AppResponse::Error(pawork_domain::ErrorContext {
                        category: pawork_domain::ErrorCategory::Internal,
                        message: error.to_string(),
                        retryable: error.retryable,
                        retry_after_ms: None,
                        diagnostics: Default::default(),
                    }),
                };
                vec![HeadlessResponse::Response {
                    envelope: wrap_response(&request_id, response),
                }]
            }
            TranslatedRequest::CompatImport(request) => {
                if let Some(error) = self.gate(
                    Some(SdkCapability::CompatImport),
                    Some(request.request_id.clone()),
                ) {
                    return vec![error];
                }
                vec![self.compat_import(request).await]
            }
            TranslatedRequest::CompatHistory(query) => {
                if let Some(error) = self.gate(
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
        if !self.granted.contains(&SdkCapability::Streaming) {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            return None;
        }
        match self.events.try_recv() {
            Ok(envelope) => Some(HeadlessResponse::Event { envelope }),
            Err(TryRecvError::Lagged(missed)) => Some(HeadlessResponse::Error {
                request_id: None,
                kind: ProtocolErrorKind::Backpressure,
                message: format!("event subscriber lagged; missed {missed}"),
            }),
            Err(TryRecvError::Empty | TryRecvError::Closed) => {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                None
            }
        }
    }
}

impl HeadlessHandler {
    async fn compat_import(
        &self,
        request: pawork_protocol::headless::CompatImportRequest,
    ) -> HeadlessResponse {
        let store = match self.store().await {
            Ok(store) => store,
            Err(message) => {
                return HeadlessResponse::Error {
                    request_id: Some(request.request_id),
                    kind: ProtocolErrorKind::Internal,
                    message,
                }
            }
        };
        let source = map_source(request.source);
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
            Err(error) => HeadlessResponse::Error {
                request_id: Some(request.request_id),
                kind: ProtocolErrorKind::CompatRejected,
                message: error.to_string(),
            },
        }
    }

    async fn compat_history(
        &self,
        query: pawork_protocol::headless::wire::CompatHistoryQuery,
    ) -> HeadlessResponse {
        let store = match self.store().await {
            Ok(store) => store,
            Err(message) => {
                return HeadlessResponse::Error {
                    request_id: Some(query.request_id),
                    kind: ProtocolErrorKind::Internal,
                    message,
                }
            }
        };
        match store
            .compat_import_history(query.limit, query.cursor.as_deref())
            .await
        {
            Ok(page) => HeadlessResponse::CompatHistoryResult {
                request_id: query.request_id,
                entries: page
                    .entries
                    .into_iter()
                    .map(|entry| CompatHistoryEntry {
                        session_id: entry.session_id,
                        source: map_source_back(entry.source),
                        original_id: entry.original_id,
                        imported_events: entry.imported_events,
                        imported_at_unix_ms: entry.imported_at_unix_ms,
                    })
                    .collect(),
                cursor: page.cursor,
            },
            Err(error) => HeadlessResponse::Error {
                request_id: Some(query.request_id),
                kind: ProtocolErrorKind::CompatRejected,
                message: error.to_string(),
            },
        }
    }
}

fn gate_capability(
    granted: &[SdkCapability],
    capability: Option<SdkCapability>,
    request_id: Option<String>,
) -> Option<HeadlessResponse> {
    match capability {
        Some(capability) if granted.contains(&capability) => None,
        Some(capability) => Some(HeadlessResponse::Error {
            request_id,
            kind: ProtocolErrorKind::UnsupportedCapability,
            message: format!("capability `{capability:?}` is not granted to this client"),
        }),
        None => Some(HeadlessResponse::Error {
            request_id,
            kind: ProtocolErrorKind::UnsupportedCapability,
            message: "command is not mapped to a capability".into(),
        }),
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_add_is_unmapped_and_fail_closed_with_compat_history() {
        let command = AppCommand::WorkspaceAdd {
            root_path: "/tmp/ws".into(),
        };
        assert!(
            command_capability(&command).is_none(),
            "WorkspaceAdd 不得静默映射到已有 capability"
        );
        let rejected = gate_capability(
            &[SdkCapability::CompatHistory],
            command_capability(&command),
            Some("req-1".into()),
        );
        assert!(
            matches!(
                rejected,
                Some(HeadlessResponse::Error {
                    kind: ProtocolErrorKind::UnsupportedCapability,
                    ..
                })
            ),
            "仅 CompatHistory 握手后 WorkspaceAdd 应 UnsupportedCapability，got {rejected:?}"
        );
    }

    #[test]
    fn granted_session_capability_still_allows_mapped_commands() {
        let command = AppCommand::SessionCreate {
            workspace_id: pawork_domain::WorkspaceId::from("ws-1"),
            title: None,
        };
        assert_eq!(command_capability(&command), Some(SdkCapability::Sessions));
        assert!(
            gate_capability(
                &[SdkCapability::Sessions],
                command_capability(&command),
                Some("req-2".into()),
            )
            .is_none()
        );
    }
}

fn map_report(report: pawork_storage::session::CompatImportReport) -> CompatImportReport {
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
