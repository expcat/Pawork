//! Agent 统一消费接口：把语言服务能力归一为 canonical 结果的九项接口。
//!
//! 九项：diagnostics / hover / definition / references / document_symbols /
//! workspace_symbols / call_hierarchy / rename / code_actions。rename / code_action
//! 的写编辑经注入的 [`WriteEditPolicy`] 审批，不直接写盘。

use std::sync::Arc;
use std::time::Duration;

use agent_domain::CancellationToken;
use serde::Serialize;
use serde_json::{json, Value};

use crate::artifact::ArtifactSink;
use crate::client::LspClient;
use crate::error::LspError;
use crate::protocol::{
    parse_range_value, CallHierarchyEdge, CallHierarchyItem, CodeAction, Diagnostic,
    DocumentSymbol, Hover, Location, MarkupKind, Position, Range, ResultPayload, WorkspaceEdit,
    WorkspaceSymbol,
};
use crate::write_policy::{
    authorize_and_apply, EditApplier, EditOrigin, EditOutcome, EditRequest, WriteEditPolicy,
};

const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 20;

/// 统一语言服务消费接口。
pub struct LanguageClient {
    client: LspClient,
    write_policy: Arc<dyn WriteEditPolicy + Send + Sync>,
    edit_applier: Option<Arc<dyn EditApplier + Send + Sync>>,
    artifact_sink: Option<Arc<dyn ArtifactSink + Send + Sync>>,
    request_timeout: Duration,
}

impl LanguageClient {
    pub fn new(client: LspClient) -> Self {
        Self {
            client,
            write_policy: Arc::new(crate::write_policy::DenyAllPolicy),
            edit_applier: None,
            artifact_sink: None,
            request_timeout: Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS),
        }
    }

    pub fn with_write_policy(mut self, policy: Arc<dyn WriteEditPolicy + Send + Sync>) -> Self {
        self.write_policy = policy;
        self
    }

    pub fn with_edit_applier(mut self, applier: Arc<dyn EditApplier + Send + Sync>) -> Self {
        self.edit_applier = Some(applier);
        self
    }

    pub fn with_artifact_sink(mut self, sink: Arc<dyn ArtifactSink + Send + Sync>) -> Self {
        self.artifact_sink = Some(sink);
        self
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    pub fn lsp(&self) -> &LspClient {
        &self.client
    }

    async fn req(
        &self,
        method: &str,
        params: Option<Value>,
        cancel: Option<&CancellationToken>,
    ) -> Result<Value, LspError> {
        self.client
            .request_value(method, params, self.request_timeout, cancel)
            .await
    }

    /// 统一 artifact fail-closed 出口：把解析后的 canonical 结果序列化，超过
    /// [`crate::artifact::ARTIFACT_INLINE_THRESHOLD`] 时必须经 [`ArtifactSink`] 落
    /// artifact 并返回引用；未注入 sink 时显式失败，绝不把大 payload 内联回流。
    async fn offload_typed<T: Serialize>(
        &self,
        kind: &str,
        value: T,
    ) -> Result<ResultPayload<T>, LspError> {
        match crate::artifact::maybe_offload(self.artifact_sink.as_deref(), kind, &value).await? {
            crate::protocol::ResultPayload::Inline(_) => Ok(ResultPayload::Inline(value)),
            crate::protocol::ResultPayload::Artifact(reference) => {
                Ok(ResultPayload::Artifact(reference))
            }
        }
    }

    /// 请求 → 解析 → artifact fail-closed：九项统一消费接口共用此路径。
    async fn parsed_payload<T: Serialize>(
        &self,
        method: &str,
        value: Value,
        parse: impl FnOnce(Value) -> Result<T, LspError>,
    ) -> Result<ResultPayload<T>, LspError> {
        self.offload_typed(method, parse(value)?).await
    }

    /// 大体积结果的 artifact 感知入口（ADR-018）。
    ///
    /// 把任意 LSP 方法的结果反序列化为 canonical 类型 `T`；序列化后超过
    /// [`ARTIFACT_INLINE_THRESHOLD`] 时必须经 [`ArtifactSink`] 落 artifact 并返回
    /// [`ResultPayload::Artifact`] 引用；未注入 sink 时显式失败，避免大 payload 回流上下文。
    /// diagnostics / 符号表 / 大范围 WorkspaceEdit 等重结果建议经此入口消费。
    pub async fn fetch_payload<T>(
        &self,
        method: &str,
        params: Option<Value>,
        cancel: Option<&CancellationToken>,
    ) -> Result<crate::protocol::ResultPayload<T>, LspError>
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
    {
        let value = self.req(method, params, cancel).await?;
        let typed: T = serde_json::from_value(value).map_err(LspError::Json)?;
        self.offload_typed(method, typed).await
    }

    // ===== 九项统一接口 =====

    /// diagnostics：返回已推送（publishDiagnostics）累积的诊断，按 uri 过滤。
    /// 每个 uri 只返回最新一次推送值。大体积诊断经 artifact fail-closed 路径。
    /// 若服务端声明 `diagnosticProvider`，也可经 [`Self::pull_diagnostics`] 拉取。
    pub async fn diagnostics(&self, uri: &str) -> Result<ResultPayload<Vec<Diagnostic>>, LspError> {
        let snap = self.client.diagnostics_snapshot().await;
        let mut out = Vec::new();
        for d in snap {
            if d.uri == uri {
                out = d.diagnostics;
                break;
            }
        }
        self.offload_typed("textDocument/publishDiagnostics", out)
            .await
    }

    /// 拉取诊断（`textDocument/diagnostic`，3.17+）。无能力时返回 Unsupported。
    pub async fn pull_diagnostics(
        &self,
        uri: &str,
        cancel: Option<&CancellationToken>,
    ) -> Result<ResultPayload<Vec<Diagnostic>>, LspError> {
        let value = self
            .req(
                "textDocument/diagnostic",
                Some(json!({ "textDocument": { "uri": uri } })),
                cancel,
            )
            .await?;
        let items = value.get("items").cloned().unwrap_or(Value::Array(vec![]));
        self.parsed_payload("textDocument/diagnostic", items, parse_diagnostics)
            .await
    }

    pub async fn hover(
        &self,
        uri: &str,
        position: Position,
        cancel: Option<&CancellationToken>,
    ) -> Result<ResultPayload<Option<Hover>>, LspError> {
        let value = self
            .req(
                "textDocument/hover",
                Some(json!({ "textDocument": { "uri": uri }, "position": position })),
                cancel,
            )
            .await?;
        self.parsed_payload("textDocument/hover", value, |v| {
            if v.is_null() {
                Ok(None)
            } else {
                Ok(Some(parse_hover(&v)?))
            }
        })
        .await
    }

    pub async fn definition(
        &self,
        uri: &str,
        position: Position,
        cancel: Option<&CancellationToken>,
    ) -> Result<ResultPayload<Vec<Location>>, LspError> {
        let value = self
            .req(
                "textDocument/definition",
                Some(json!({ "textDocument": { "uri": uri }, "position": position })),
                cancel,
            )
            .await?;
        self.parsed_payload("textDocument/definition", value, parse_locations)
            .await
    }

    pub async fn references(
        &self,
        uri: &str,
        position: Position,
        include_declaration: bool,
        cancel: Option<&CancellationToken>,
    ) -> Result<ResultPayload<Vec<Location>>, LspError> {
        let value = self
            .req(
                "textDocument/references",
                Some(json!({
                    "textDocument": { "uri": uri },
                    "position": position,
                    "context": { "includeDeclaration": include_declaration }
                })),
                cancel,
            )
            .await?;
        self.parsed_payload("textDocument/references", value, parse_locations)
            .await
    }

    pub async fn document_symbols(
        &self,
        uri: &str,
        cancel: Option<&CancellationToken>,
    ) -> Result<ResultPayload<Vec<DocumentSymbol>>, LspError> {
        let value = self
            .req(
                "textDocument/documentSymbol",
                Some(json!({ "textDocument": { "uri": uri } })),
                cancel,
            )
            .await?;
        self.parsed_payload(
            "textDocument/documentSymbol",
            value,
            normalize_document_symbols,
        )
        .await
    }

    pub async fn workspace_symbols(
        &self,
        query: &str,
        cancel: Option<&CancellationToken>,
    ) -> Result<ResultPayload<Vec<WorkspaceSymbol>>, LspError> {
        let value = self
            .req("workspace/symbol", Some(json!({ "query": query })), cancel)
            .await?;
        self.parsed_payload("workspace/symbol", value, |v| {
            serde_json::from_value::<Vec<WorkspaceSymbol>>(v).map_err(LspError::Json)
        })
        .await
    }

    pub async fn prepare_call_hierarchy(
        &self,
        uri: &str,
        position: Position,
        cancel: Option<&CancellationToken>,
    ) -> Result<ResultPayload<Vec<CallHierarchyItem>>, LspError> {
        let value = self
            .req(
                "textDocument/prepareCallHierarchy",
                Some(json!({ "textDocument": { "uri": uri }, "position": position })),
                cancel,
            )
            .await?;
        self.parsed_payload("textDocument/prepareCallHierarchy", value, |v| {
            serde_json::from_value::<Vec<CallHierarchyItem>>(v).map_err(LspError::Json)
        })
        .await
    }

    pub async fn incoming_calls(
        &self,
        item: &CallHierarchyItem,
        cancel: Option<&CancellationToken>,
    ) -> Result<ResultPayload<Vec<CallHierarchyEdge>>, LspError> {
        let value = self
            .req(
                "callHierarchy/incomingCalls",
                Some(json!({ "item": item })),
                cancel,
            )
            .await?;
        self.parsed_payload("callHierarchy/incomingCalls", value, parse_call_edges)
            .await
    }

    pub async fn outgoing_calls(
        &self,
        item: &CallHierarchyItem,
        cancel: Option<&CancellationToken>,
    ) -> Result<ResultPayload<Vec<CallHierarchyEdge>>, LspError> {
        let value = self
            .req(
                "callHierarchy/outgoingCalls",
                Some(json!({ "item": item })),
                cancel,
            )
            .await?;
        self.parsed_payload("callHierarchy/outgoingCalls", value, parse_call_edges)
            .await
    }

    /// rename：返回规范化 WorkspaceEdit（不写盘）。
    pub async fn rename(
        &self,
        uri: &str,
        position: Position,
        new_name: &str,
        cancel: Option<&CancellationToken>,
    ) -> Result<ResultPayload<WorkspaceEdit>, LspError> {
        let value = self
            .req(
                "textDocument/rename",
                Some(json!({
                    "textDocument": { "uri": uri },
                    "position": position,
                    "newName": new_name
                })),
                cancel,
            )
            .await?;
        self.parsed_payload("textDocument/rename", value, |v| parse_workspace_edit(&v))
            .await
    }

    pub async fn code_actions(
        &self,
        uri: &str,
        range: Range,
        cancel: Option<&CancellationToken>,
    ) -> Result<ResultPayload<Vec<CodeAction>>, LspError> {
        let value = self
            .req(
                "textDocument/codeAction",
                Some(json!({
                    "textDocument": { "uri": uri },
                    "range": range,
                    "context": { "diagnostics": [] }
                })),
                cancel,
            )
            .await?;
        self.parsed_payload("textDocument/codeAction", value, |v| {
            serde_json::from_value::<Vec<CodeAction>>(v).map_err(LspError::Json)
        })
        .await
    }

    // ===== 写操作策略 =====

    /// 把 rename / code_action 的 WorkspaceEdit 经策略审批并（如注入了 applier）落盘。
    /// 语言服务本身不直接写盘；`Allow` 但未注入 applier 时返回
    /// [`LspError::NoEditApplier`]，`Ask` 返回 [`EditOutcome::PendingUserConfirmation`]。
    pub async fn apply_edit(
        &self,
        origin: EditOrigin,
        edit: WorkspaceEdit,
    ) -> Result<EditOutcome, LspError> {
        let request = EditRequest {
            origin,
            descriptor_id: self.client.descriptor().id.clone(),
            workspace: self
                .client
                .inner()
                .spawn_config
                .workspace_roots
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            edit,
        };
        authorize_and_apply(
            self.write_policy.as_ref(),
            self.edit_applier.as_deref(),
            &request,
        )
        .await
    }

    // ===== 文档同步 =====

    pub async fn did_open(&self, uri: &str, language_id: &str, text: &str) -> Result<(), LspError> {
        let params = self
            .client
            .with_docs(|docs| docs.open(uri, language_id, text))
            .await;
        self.client
            .notify("textDocument/didOpen", Some(params))
            .await
    }

    pub async fn did_change_full(&self, uri: &str, new_text: &str) -> Result<(), LspError> {
        let params = self
            .client
            .with_docs(|docs| docs.change_full(uri, new_text))
            .await
            .ok_or_else(|| LspError::InvalidState(format!("document not open: {uri}")))?;
        self.client
            .notify("textDocument/didChange", Some(params))
            .await
    }

    pub async fn did_change_incremental(
        &self,
        uri: &str,
        range: Range,
        new_text: &str,
    ) -> Result<(), LspError> {
        let params = self
            .client
            .with_docs(|docs| docs.change_incremental(uri, range, new_text))
            .await
            .ok_or_else(|| LspError::InvalidState(format!("document not open: {uri}")))?;
        self.client
            .notify("textDocument/didChange", Some(params))
            .await
    }

    pub async fn did_close(&self, uri: &str) -> Result<(), LspError> {
        let params = self
            .client
            .with_docs(|docs| docs.close(uri))
            .await
            .ok_or_else(|| LspError::InvalidState(format!("document not open: {uri}")))?;
        self.client
            .notify("textDocument/didClose", Some(params))
            .await
    }

    pub async fn shutdown(self) -> Result<(), LspError> {
        self.client.shutdown().await
    }
}

// ===== 解析归一助手 =====

fn parse_locations(value: Value) -> Result<Vec<Location>, LspError> {
    match value {
        Value::Null => Ok(Vec::new()),
        Value::Array(_) => serde_json::from_value::<Vec<Location>>(value).map_err(LspError::Json),
        Value::Object(_) => serde_json::from_value::<Location>(value)
            .map(|l| vec![l])
            .map_err(LspError::Json),
        _ => Err(LspError::Json(serde::de::Error::custom(
            "unexpected location shape",
        ))),
    }
}

fn parse_diagnostics(value: Value) -> Result<Vec<Diagnostic>, LspError> {
    serde_json::from_value::<Vec<Diagnostic>>(value).map_err(LspError::Json)
}

fn parse_hover(value: &Value) -> Result<Hover, LspError> {
    let content = value.get("contents").or(Some(value));
    let (text, kind) = match content.and_then(|c| c.as_str()) {
        Some(s) => (s.to_string(), MarkupKind::PlainText),
        None => {
            let obj = value
                .get("contents")
                .and_then(|c| c.as_object())
                .ok_or_else(|| LspError::Transport("hover missing contents".into()))?;
            let value = obj
                .get("value")
                .and_then(|v| v.as_str())
                .ok_or_else(|| LspError::Transport("hover contents missing value".into()))?
                .to_string();
            let kind = match obj.get("kind").and_then(|k| k.as_str()) {
                Some("markdown") => MarkupKind::Markdown,
                _ => MarkupKind::PlainText,
            };
            (value, kind)
        }
    };
    let range = value.get("range").and_then(parse_range_value);
    Ok(Hover {
        content: text,
        kind,
        range,
    })
}

fn parse_call_edges(value: Value) -> Result<Vec<CallHierarchyEdge>, LspError> {
    serde_json::from_value::<Vec<CallHierarchyEdge>>(value).map_err(LspError::Json)
}

fn parse_workspace_edit(value: &Value) -> Result<WorkspaceEdit, LspError> {
    // 统一经 protocol 的 wire 解析：TextDocumentEdit + Create/Rename/Delete 文件操作
    // 全部显式建模，无法建模的条目返回错误，不静默丢弃。
    crate::protocol::workspace_edit_from_value(value)
        .map_err(|msg| LspError::Json(serde::de::Error::custom(msg)))
}

/// LSP `documentSymbol` 响应可能是层级 `DocumentSymbol[]`（带 range/selectionRange）
/// 或扁平 `SymbolInformation[]`（带 location）；统一归一为层级结构。
fn normalize_document_symbols(value: Value) -> Result<Vec<DocumentSymbol>, LspError> {
    let arr = value.as_array().ok_or_else(|| {
        LspError::Json(serde::de::Error::custom(
            "documentSymbol response is not an array",
        ))
    })?;
    let flat = arr
        .first()
        .map(|e| e.get("location").is_some())
        .unwrap_or(false);
    if !flat {
        return serde_json::from_value::<Vec<DocumentSymbol>>(value).map_err(LspError::Json);
    }
    // 扁平 SymbolInformation[] → 单层 DocumentSymbol：range = selectionRange =
    // location.range，containerName → detail。
    let infos = serde_json::from_value::<Vec<SymbolInformation>>(value).map_err(LspError::Json)?;
    Ok(infos
        .into_iter()
        .map(|i| DocumentSymbol {
            name: i.name,
            kind: i.kind,
            range: i.location.range,
            selection_range: i.location.range,
            detail: i.container_name,
            children: Vec::new(),
        })
        .collect())
}

/// LSP `SymbolInformation`（扁平 documentSymbol / workspaceSymbol 的 wire 形态）。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SymbolInformation {
    name: String,
    kind: crate::protocol::SymbolKind,
    location: Location,
    #[serde(default)]
    container_name: Option<String>,
}
