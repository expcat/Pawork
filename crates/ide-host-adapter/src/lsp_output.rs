//! 可选 LSP Server 输出映射（P17-9 步骤 5）。
//!
//! 当 IDE 仅需消费 Pawork 聚合的代码智能时，本模块把 P17-4 LSP Client
//! 聚合结果（diagnostics / hover / definition / references）映射为 LSP
//! JSON-RPC 消息面。**只做映射**：不运行语言服务、不改变 P17-4 作为
//! LSP Client 的主定位。
//!
//! 执行入口：`IdeHostAdapter` 的 `LspQuery` 请求经注入的
//! [`LspResultProvider`] 消费聚合结果（宿主侧用 `lsp-runtime::LanguageClient`
//! 实现），本模块负责 canonical → LSP wire 的编码。

use async_trait::async_trait;
use lsp_runtime::{Diagnostic, Hover, Location, MarkupKind};
use serde_json::{json, Value};

use crate::contract::LspQueryKind;

/// 可选 LSP 输出的结果提供方（宿主侧注入；本 crate 不运行语言服务）。
#[async_trait]
pub trait LspResultProvider: Send + Sync {
    /// 解析一个消费类查询为 LSP wire 结果；失败返回显式错误文本。
    async fn resolve(&self, query: &LspQueryKind) -> Result<Value, String>;
}

/// canonical 聚合结果 → LSP JSON-RPC 消息编码器（纯映射）。
#[derive(Clone, Copy, Debug, Default)]
pub struct LspOutputEncoder;

impl LspOutputEncoder {
    pub fn new() -> Self {
        Self
    }

    /// `textDocument/publishDiagnostics` 通知（P17-4 聚合结果 → IDE）。
    pub fn publish_diagnostics(
        &self,
        uri: &str,
        version: Option<i64>,
        diagnostics: &[Diagnostic],
    ) -> Value {
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": uri,
                "version": version,
                "diagnostics": diagnostics,
            }
        })
    }

    /// `textDocument/hover` 响应结果（canonical `Hover` → LSP wire）。
    pub fn hover_result(&self, hover: &Hover) -> Value {
        let kind = match hover.kind {
            MarkupKind::Markdown => "markdown",
            MarkupKind::PlainText => "plaintext",
        };
        json!({
            "contents": { "kind": kind, "value": hover.content },
            "range": hover.range,
        })
    }

    /// `textDocument/definition` / `references` 响应结果（`Location` 列表）。
    pub fn locations_result(&self, locations: &[Location]) -> Value {
        json!(locations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_runtime::{DiagnosticSeverity, Position, Range};

    #[test]
    fn publish_diagnostics_is_jsonrpc_notification() {
        let encoder = LspOutputEncoder::new();
        let diagnostic = Diagnostic {
            range: Range::new(Position::new(0, 0), Position::new(0, 1)),
            severity: Some(DiagnosticSeverity::Error),
            code: Some(serde_json::json!("E0001")),
            source: Some("rust-analyzer".into()),
            message: "boom".into(),
        };
        let message = encoder.publish_diagnostics("file:///a.rs", Some(2), &[diagnostic]);
        assert_eq!(message["jsonrpc"], "2.0");
        assert_eq!(message["method"], "textDocument/publishDiagnostics");
        assert_eq!(message["params"]["uri"], "file:///a.rs");
        assert_eq!(message["params"]["version"], 2);
        assert_eq!(message["params"]["diagnostics"][0]["severity"], 1);
        assert_eq!(
            message["params"]["diagnostics"][0]["range"]["start"]["line"],
            0
        );
        assert_eq!(message["params"]["diagnostics"][0]["message"], "boom");
    }

    #[test]
    fn hover_and_locations_map_to_lsp_wire() {
        let encoder = LspOutputEncoder::new();
        let hover = Hover {
            content: "fn main".into(),
            kind: MarkupKind::PlainText,
            range: Some(Range::new(Position::new(1, 0), Position::new(1, 8))),
        };
        let result = encoder.hover_result(&hover);
        assert_eq!(result["contents"]["kind"], "plaintext");
        assert_eq!(result["contents"]["value"], "fn main");
        assert_eq!(result["range"]["start"]["line"], 1);

        let locations = vec![Location {
            uri: "file:///a.rs".into(),
            range: Range::new(Position::new(3, 4), Position::new(3, 9)),
        }];
        let result = encoder.locations_result(&locations);
        assert_eq!(result[0]["uri"], "file:///a.rs");
        assert_eq!(result[0]["range"]["end"]["character"], 9);
    }
}
