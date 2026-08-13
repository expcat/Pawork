//! 诊断双向回灌映射（P17-9 步骤 2）。
//!
//! - 出向：P17-4 LSP Client 聚合的 [`DocumentDiagnostic`] → 契约
//!   [`IdeDiagnosticSet`]（IDE 展示）。
//! - 反向：IDE 显示的诊断变化（[`crate::IdeRequest::DiagnosticsPublish`]）→
//!   [`DiagnosticBoard`] canonical 变更记录（供 Agent 闭环消费）。
//!
//! Policy 边界：本模块**只映射与记录**，不写文件、不应用编辑、不绕过
//! Policy（见 [`DIAGNOSTIC_POLICY_BOUNDARY`]）；诊断的写入路径必须回到
//! Core 既有命令面（`AppCommand::RunTool` 等），由 Core 侧 Policy 审批。

use std::collections::BTreeMap;

use core_api::{ClientDiagnostic, ClientDiagnosticSeverity};
use lsp_runtime::{Diagnostic, DocumentDiagnostic};

use crate::contract::{IdeDiagnostic, IdeEvent};

/// 诊断映射的 Policy 边界声明（Adapter 侧只映射，无文件系统/写权限面）。
pub const DIAGNOSTIC_POLICY_BOUNDARY: &str =
    "ide-host-adapter maps diagnostics only; it never writes files or bypasses policy";

impl From<&Diagnostic> for IdeDiagnostic {
    fn from(diagnostic: &Diagnostic) -> Self {
        Self {
            range: diagnostic.range,
            severity: diagnostic.severity,
            code: diagnostic.code.as_ref().and_then(|code| match code {
                serde_json::Value::String(value) => Some(value.clone()),
                serde_json::Value::Null => None,
                other => Some(other.to_string()),
            }),
            source: diagnostic.source.clone(),
            message: diagnostic.message.clone(),
        }
    }
}

/// 一个文档的诊断集合（契约形态）。
#[derive(Clone, Debug, PartialEq)]
pub struct IdeDiagnosticSet {
    pub document_uri: String,
    pub version: Option<i64>,
    pub diagnostics: Vec<IdeDiagnostic>,
}

impl From<&DocumentDiagnostic> for IdeDiagnosticSet {
    fn from(document: &DocumentDiagnostic) -> Self {
        Self {
            document_uri: document.uri.clone(),
            version: document.version,
            diagnostics: document
                .diagnostics
                .iter()
                .map(IdeDiagnostic::from)
                .collect(),
        }
    }
}

impl From<&IdeDiagnosticSet> for IdeEvent {
    fn from(set: &IdeDiagnosticSet) -> Self {
        IdeEvent::DiagnosticsChanged {
            document_uri: set.document_uri.clone(),
            version: set.version,
            diagnostics: set.diagnostics.clone(),
        }
    }
}

/// 诊断看板：uri → 最近诊断集合；出向快照与反向变更都落在这里。
#[derive(Clone, Debug, Default)]
pub struct DiagnosticBoard {
    docs: BTreeMap<String, IdeDiagnosticSet>,
}

impl DiagnosticBoard {
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用 P17-4 LSP Client 聚合结果；对变化的文档发出
    /// `DiagnosticsChanged` 事件（IDE 展示）。
    pub fn apply_lsp_snapshot(&mut self, snapshots: &[DocumentDiagnostic]) -> Vec<IdeEvent> {
        let mut events = Vec::new();
        for document in snapshots {
            let set = IdeDiagnosticSet::from(document);
            let changed = self.docs.get(&set.document_uri) != Some(&set);
            if changed {
                self.docs.insert(set.document_uri.clone(), set.clone());
                events.push(IdeEvent::from(&set));
            }
        }
        events
    }

    /// 反向回灌：IDE 显示的诊断变化 → canonical 变更记录；同时向扩展
    /// 回发确认事件。只更新看板，不产生任何文件系统副作用。
    pub fn apply_ide_publish(&mut self, set: IdeDiagnosticSet) -> Vec<IdeEvent> {
        let changed = self.docs.get(&set.document_uri) != Some(&set);
        if !changed {
            return Vec::new();
        }
        self.docs.insert(set.document_uri.clone(), set.clone());
        vec![IdeEvent::from(&set)]
    }

    pub fn get(&self, uri: &str) -> Option<&IdeDiagnosticSet> {
        self.docs.get(uri)
    }

    /// 当前全量快照（供扩展/宿主在 run 上下文或闭环消费点使用）。
    pub fn snapshot(&self) -> Vec<IdeDiagnosticSet> {
        self.docs.values().cloned().collect()
    }

    /// 展平为 Core canonical 诊断；仍是纯观察数据，不获得写入或工具权限。
    pub fn client_diagnostics(&self) -> Vec<ClientDiagnostic> {
        self.docs
            .values()
            .flat_map(|set| {
                set.diagnostics.iter().map(|diagnostic| ClientDiagnostic {
                    document_uri: set.document_uri.clone(),
                    version: set.version,
                    range: crate::lifecycle::client_range(diagnostic.range),
                    severity: diagnostic.severity.map(|severity| match severity {
                        lsp_runtime::DiagnosticSeverity::Error => ClientDiagnosticSeverity::Error,
                        lsp_runtime::DiagnosticSeverity::Warning => {
                            ClientDiagnosticSeverity::Warning
                        }
                        lsp_runtime::DiagnosticSeverity::Information => {
                            ClientDiagnosticSeverity::Information
                        }
                        lsp_runtime::DiagnosticSeverity::Hint => ClientDiagnosticSeverity::Hint,
                    }),
                    code: diagnostic.code.clone(),
                    source: diagnostic.source.clone(),
                    message: diagnostic.message.clone(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_runtime::{DiagnosticSeverity, Position, Range};

    fn diagnostic(severity: DiagnosticSeverity) -> Diagnostic {
        Diagnostic {
            range: Range::new(Position::new(0, 0), Position::new(0, 4)),
            severity: Some(severity),
            code: Some(serde_json::json!(42)),
            source: Some("rust-analyzer".into()),
            message: "unused variable".into(),
        }
    }

    #[test]
    fn lsp_diagnostic_maps_to_contract_shape() {
        let mapped = IdeDiagnostic::from(&diagnostic(DiagnosticSeverity::Warning));
        assert_eq!(mapped.range.start.line, 0);
        assert_eq!(mapped.range.end.character, 4);
        assert_eq!(mapped.severity, Some(DiagnosticSeverity::Warning));
        assert_eq!(mapped.code.as_deref(), Some("42"));
        assert_eq!(mapped.source.as_deref(), Some("rust-analyzer"));
        assert_eq!(mapped.message, "unused variable");
    }

    #[test]
    fn board_applies_lsp_snapshot_and_publishes_changes() {
        let mut board = DiagnosticBoard::new();
        let document = DocumentDiagnostic {
            uri: "file:///a.rs".into(),
            version: Some(3),
            diagnostics: vec![diagnostic(DiagnosticSeverity::Error)],
        };

        let events = board.apply_lsp_snapshot(std::slice::from_ref(&document));
        assert_eq!(events.len(), 1, "changed document emits one event");
        let IdeEvent::DiagnosticsChanged {
            document_uri,
            version,
            diagnostics,
        } = &events[0]
        else {
            panic!("unexpected event");
        };
        assert_eq!(document_uri, "file:///a.rs");
        assert_eq!(*version, Some(3));
        assert_eq!(diagnostics.len(), 1);

        // 幂等：相同快照不重复发事件。
        let events = board.apply_lsp_snapshot(&[document]);
        assert!(events.is_empty());
    }

    #[test]
    fn reverse_publish_records_change_without_fs_side_effects() {
        let mut board = DiagnosticBoard::new();
        let set = IdeDiagnosticSet {
            document_uri: "file:///b.rs".into(),
            version: Some(1),
            diagnostics: vec![IdeDiagnostic::from(&diagnostic(DiagnosticSeverity::Hint))],
        };

        let events = board.apply_ide_publish(set.clone());
        assert_eq!(events.len(), 1);
        assert_eq!(board.get("file:///b.rs"), Some(&set));
        assert_eq!(board.snapshot().len(), 1);
        // 反向回灌只更新看板（映射/记录），不触碰文件系统。
        assert_eq!(
            board.snapshot()[0].diagnostics[0].message,
            "unused variable"
        );
        assert_eq!(
            DIAGNOSTIC_POLICY_BOUNDARY,
            "ide-host-adapter maps diagnostics only; it never writes files or bypasses policy"
        );

        let events = board.apply_ide_publish(set.clone());
        assert!(events.is_empty(), "identical reverse publish is idempotent");
        assert_eq!(board.get("file:///b.rs"), Some(&set));
    }
}
