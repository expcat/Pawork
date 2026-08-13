//! 服务端能力协商：`initialize` 响应里的 `ServerCapabilities` 子集 + 能力查询。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::MarkupKind;

/// 客户端在 `initialize` 中声明的能力子集。
#[derive(Debug, Clone, Default, Serialize)]
pub struct ClientCapabilities {
    pub hover_markup: Vec<MarkupKind>,
    pub supports_pull_diagnostics: bool,
    pub incremental_change: bool,
}

impl ClientCapabilities {
    pub fn pawork_default() -> Self {
        Self {
            hover_markup: vec![MarkupKind::Markdown, MarkupKind::PlainText],
            supports_pull_diagnostics: true,
            incremental_change: true,
        }
    }

    pub fn to_lsp(&self) -> Value {
        serde_json::json!({
            "capabilities": {
                "textDocument": {
                    "synchronization": {
                        "didSave": true,
                        "willSave": false,
                        "dynamicRegistration": false,
                    },
                    "hover": {
                        "contentFormat": self.hover_markup.iter().map(|m| match m {
                            MarkupKind::Markdown => "markdown",
                            MarkupKind::PlainText => "plaintext",
                        }).collect::<Vec<_>>()
                    }
                },
                "workspace": {}
            }
        })
    }
}

/// `initialize` 响应中的 `ServerCapabilities` 子集。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ServerCapabilities {
    #[serde(default)]
    pub text_document_sync: Option<TextDocumentSyncCapability>,
    #[serde(default)]
    pub hover_provider: Option<bool>,
    #[serde(default)]
    pub definition_provider: Option<bool>,
    #[serde(default)]
    pub references_provider: Option<bool>,
    #[serde(default)]
    pub document_symbol_provider: Option<bool>,
    #[serde(default)]
    pub workspace_symbol_provider: Option<bool>,
    #[serde(default)]
    pub call_hierarchy_provider: Option<bool>,
    #[serde(default)]
    pub rename_provider: Option<bool>,
    #[serde(default)]
    pub code_action_provider: Option<bool>,
    #[serde(default)]
    pub diagnostic_provider: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum TextDocumentSyncCapability {
    Kind(i64),
    Options(TextDocumentSyncOptions),
}

impl TextDocumentSyncCapability {
    pub fn incremental(&self) -> bool {
        match self {
            TextDocumentSyncCapability::Kind(k) => *k == 2,
            TextDocumentSyncCapability::Options(o) => {
                o.open_close.unwrap_or(false) && o.change.unwrap_or(0) == 2
            }
        }
    }

    pub fn open_close(&self) -> bool {
        match self {
            TextDocumentSyncCapability::Kind(_) => true,
            TextDocumentSyncCapability::Options(o) => o.open_close.unwrap_or(false),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct TextDocumentSyncOptions {
    #[serde(default)]
    pub open_close: Option<bool>,
    #[serde(default)]
    pub change: Option<i64>,
    #[serde(default)]
    pub save: Option<bool>,
}

/// 能力查询：给定方法名 + 已协商能力，判定是否支持。
pub fn method_supported(caps: &ServerCapabilities, method: &str) -> bool {
    match method {
        "textDocument/hover" => caps.hover_provider.unwrap_or(false),
        "textDocument/definition" => caps.definition_provider.unwrap_or(false),
        "textDocument/references" => caps.references_provider.unwrap_or(false),
        "textDocument/documentSymbol" => caps.document_symbol_provider.unwrap_or(false),
        "workspace/symbol" => caps.workspace_symbol_provider.unwrap_or(false),
        "textDocument/prepareCallHierarchy"
        | "callHierarchy/incomingCalls"
        | "callHierarchy/outgoingCalls" => caps.call_hierarchy_provider.unwrap_or(false),
        "textDocument/rename" | "textDocument/prepareRename" => {
            caps.rename_provider.unwrap_or(false)
        }
        "textDocument/codeAction" => caps.code_action_provider.unwrap_or(false),
        "textDocument/diagnostic" => caps.diagnostic_provider.unwrap_or(false),
        _ => true,
    }
}

/// 规范化服务端能力（处理 `*Provider` 可能是 bool 或对象的情况）。
pub fn normalize_capabilities(raw: &Value) -> ServerCapabilities {
    let mut caps = ServerCapabilities::default();
    if let Some(obj) = raw.as_object() {
        if let Some(v) = obj.get("textDocumentSync") {
            caps.text_document_sync = serde_json::from_value(v.clone()).ok();
        }
        caps.hover_provider = provider_bool(obj.get("hoverProvider"));
        caps.definition_provider = provider_bool(obj.get("definitionProvider"));
        caps.references_provider = provider_bool(obj.get("referencesProvider"));
        caps.document_symbol_provider = provider_bool(obj.get("documentSymbolProvider"));
        caps.workspace_symbol_provider = provider_bool(obj.get("workspaceSymbolProvider"));
        caps.call_hierarchy_provider = provider_bool(obj.get("callHierarchyProvider"));
        caps.rename_provider = provider_bool(obj.get("renameProvider"));
        caps.code_action_provider = provider_bool(obj.get("codeActionProvider"));
        caps.diagnostic_provider = provider_bool(obj.get("diagnosticProvider"));
    }
    caps
}

fn provider_bool(v: Option<&Value>) -> Option<bool> {
    match v {
        Some(Value::Bool(b)) => Some(*b),
        Some(Value::Object(_)) => Some(true),
        _ => None,
    }
}
