//! 规范化的 LSP 数据模型子集（九项统一消费接口用到的类型）。
//!
//! 这些类型屏蔽具体语言服务差异，Agent 侧只消费 canonical 结构。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 文本位置（0-based line / character）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

impl Position {
    pub const fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

/// 半开区间范围 `[start, end)`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl Range {
    pub const fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }
}

/// 文档位置：URI + Range。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

/// 文本文档标识。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextDocumentIdentifier {
    pub uri: String,
}

/// `textDocument/hover` 的规范化结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hover {
    /// 可读文本（已从 MarkupContent / MarkedString 归一为单一字符串）。
    pub content: String,
    /// 归一后的 markup kind：`"markdown"` 或 `"plaintext"`。
    pub kind: MarkupKind,
    pub range: Option<Range>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkupKind {
    PlainText,
    Markdown,
}

/// 诊断严重度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "i32", from = "i32")]
pub enum DiagnosticSeverity {
    Error = 1,
    Warning = 2,
    Information = 3,
    Hint = 4,
}

impl From<DiagnosticSeverity> for i32 {
    fn from(s: DiagnosticSeverity) -> Self {
        s as i32
    }
}

impl From<i32> for DiagnosticSeverity {
    fn from(v: i32) -> Self {
        match v {
            1 => DiagnosticSeverity::Error,
            2 => DiagnosticSeverity::Warning,
            3 => DiagnosticSeverity::Information,
            4 => DiagnosticSeverity::Hint,
            _ => DiagnosticSeverity::Information,
        }
    }
}

/// 一条诊断。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub range: Range,
    pub severity: Option<DiagnosticSeverity>,
    pub code: Option<Value>,
    pub source: Option<String>,
    pub message: String,
}

/// 一个文档的诊断集合（来自 `textDocument/publishDiagnostics`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentDiagnostic {
    pub uri: String,
    pub version: Option<i64>,
    pub diagnostics: Vec<Diagnostic>,
}

/// 文档符号。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub range: Range,
    pub selection_range: Range,
    pub detail: Option<String>,
    #[serde(default)]
    pub children: Vec<DocumentSymbol>,
}

/// 工作区符号。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub location: Location,
    pub container_name: Option<String>,
}

/// LSP SymbolKind 子集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "i32", from = "i32")]
pub enum SymbolKind {
    File = 1,
    Module = 2,
    Namespace = 3,
    Class = 5,
    Method = 6,
    Property = 7,
    Field = 8,
    Constructor = 9,
    Enum = 10,
    Interface = 11,
    Function = 12,
    Variable = 13,
    Constant = 14,
    Struct = 23,
    EnumMember = 22,
}

impl From<SymbolKind> for i32 {
    fn from(k: SymbolKind) -> Self {
        k as i32
    }
}

impl From<i32> for SymbolKind {
    fn from(v: i32) -> Self {
        match v {
            1 => SymbolKind::File,
            2 => SymbolKind::Module,
            3 => SymbolKind::Namespace,
            5 => SymbolKind::Class,
            6 => SymbolKind::Method,
            7 => SymbolKind::Property,
            8 => SymbolKind::Field,
            9 => SymbolKind::Constructor,
            10 => SymbolKind::Enum,
            11 => SymbolKind::Interface,
            12 => SymbolKind::Function,
            13 => SymbolKind::Variable,
            14 => SymbolKind::Constant,
            22 => SymbolKind::EnumMember,
            23 => SymbolKind::Struct,
            _ => SymbolKind::Variable,
        }
    }
}

/// 调用层级项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallHierarchyItem {
    pub name: String,
    pub kind: SymbolKind,
    pub uri: String,
    pub range: Range,
    pub selection_range: Range,
}

/// 一条调用层级边。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallHierarchyEdge {
    pub item: CallHierarchyItem,
    pub from_ranges: Vec<Range>,
}

/// `textDocument/rename` 规范化的文本编辑。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

/// 单个文档上的编辑集合。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentEdit {
    pub uri: String,
    pub version: Option<i64>,
    pub edits: Vec<TextEdit>,
}

impl<'de> serde::Deserialize<'de> for TextDocumentEdit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // LSP wire 形态是 `{ "textDocument": { "uri", "version" }, "edits": [...] }`，
        // canonical 结构把 uri/version 展平；这里只处理 wire → canonical 方向。
        #[derive(serde::Deserialize)]
        struct WireTextDocument {
            uri: String,
            #[serde(default)]
            version: Option<i64>,
        }
        #[derive(serde::Deserialize)]
        struct Wire {
            #[serde(rename = "textDocument")]
            text_document: WireTextDocument,
            #[serde(default)]
            edits: Vec<TextEdit>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(TextDocumentEdit {
            uri: wire.text_document.uri,
            version: wire.text_document.version,
            edits: wire.edits,
        })
    }
}

/// LSP 文件操作的可选选项。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOperationOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overwrite: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignore_if_exists: Option<bool>,
}

/// `documentChanges` 中的创建文件操作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateFile {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<FileOperationOptions>,
}

/// `documentChanges` 中的重命名文件操作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameFile {
    pub old_uri: String,
    pub new_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<FileOperationOptions>,
}

/// `documentChanges` 中的删除文件操作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteFile {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<FileOperationOptions>,
}

/// LSP `documentChanges` 条目中的文件操作（create / rename / delete）。
///
/// wire 形态为 `{ "kind": "create"|"rename"|"delete", ... }`。文件操作被显式建模，
/// 由注入的 [`crate::write_policy::EditApplier`] 决定如何落盘，绝不静默丢弃。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FileOperation {
    Create(CreateFile),
    Rename(RenameFile),
    Delete(DeleteFile),
}

/// `textDocument/rename` / `codeAction` 产出的规范化工作区编辑。
///
/// 覆盖 LSP WorkspaceEdit 的全部三类内容：`changes` 文本编辑映射、
/// `documentChanges` 中的 TextDocumentEdit 与 CreateFile / RenameFile / DeleteFile
/// 文件操作。解析器遇到无法建模的条目返回错误，不静默丢弃。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkspaceEdit {
    pub document_changes: Vec<TextDocumentEdit>,
    /// 文件操作（create / rename / delete），wire 形态为 `documentChanges` 中带
    /// `kind` 的条目。
    pub file_operations: Vec<FileOperation>,
}

impl WorkspaceEdit {
    pub fn is_empty(&self) -> bool {
        self.document_changes.iter().all(|d| d.edits.is_empty()) && self.file_operations.is_empty()
    }

    /// 编辑总数：文本编辑 + 文件操作。
    pub fn total_edits(&self) -> usize {
        self.document_changes
            .iter()
            .map(|d| d.edits.len())
            .sum::<usize>()
            + self.file_operations.len()
    }
}

impl serde::Serialize for WorkspaceEdit {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(1))?;
        let mut items: Vec<Value> =
            Vec::with_capacity(self.file_operations.len() + self.document_changes.len());
        for op in &self.file_operations {
            items.push(serde_json::to_value(op).map_err(serde::ser::Error::custom)?);
        }
        for tde in &self.document_changes {
            items.push(serde_json::json!({
                "textDocument": {
                    "uri": tde.uri,
                    "version": tde.version,
                },
                "edits": tde.edits,
            }));
        }
        map.serialize_entry("documentChanges", &items)?;
        map.end()
    }
}

impl<'de> serde::Deserialize<'de> for WorkspaceEdit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        workspace_edit_from_value(&value).map_err(serde::de::Error::custom)
    }
}

/// 从 wire JSON 解析 WorkspaceEdit：`documentChanges`（TextDocumentEdit + 文件操作）
/// 优先，否则回退 `changes` 映射。无法建模的条目返回 Err，绝不静默丢弃。
pub(crate) fn workspace_edit_from_value(value: &Value) -> Result<WorkspaceEdit, String> {
    let mut document_changes = Vec::new();
    let mut file_operations = Vec::new();

    if let Some(changes) = value.get("documentChanges").and_then(|v| v.as_array()) {
        for item in changes {
            if item.get("kind").and_then(|k| k.as_str()).is_some() {
                let op = serde_json::from_value::<FileOperation>(item.clone())
                    .map_err(|e| format!("invalid file operation in documentChanges: {e}"))?;
                file_operations.push(op);
            } else {
                let tde = text_document_edit_from_value(item).ok_or_else(|| {
                    "documentChanges entry is neither a text-document edit nor a file operation"
                        .to_string()
                })?;
                document_changes.push(tde);
            }
        }
    } else if let Some(obj) = value.get("changes").and_then(|v| v.as_object()) {
        for (uri, arr) in obj {
            let edits = arr
                .as_array()
                .ok_or_else(|| format!("changes[{uri}] is not an array"))?;
            let mut parsed = Vec::with_capacity(edits.len());
            for e in edits {
                let range = e
                    .get("range")
                    .and_then(parse_range_value)
                    .ok_or_else(|| format!("changes[{uri}] edit missing range"))?;
                let new_text = e
                    .get("newText")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| format!("changes[{uri}] edit missing newText"))?
                    .to_string();
                parsed.push(TextEdit { range, new_text });
            }
            document_changes.push(TextDocumentEdit {
                uri: uri.clone(),
                version: None,
                edits: parsed,
            });
        }
    }
    Ok(WorkspaceEdit {
        document_changes,
        file_operations,
    })
}

fn text_document_edit_from_value(v: &Value) -> Option<TextDocumentEdit> {
    let uri = v.get("textDocument")?.get("uri")?.as_str()?.to_string();
    let version = v
        .get("textDocument")
        .and_then(|td| td.get("version"))
        .and_then(|v| v.as_i64());
    let edits = v
        .get("edits")
        .and_then(|e| e.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    let range = parse_range_value(e.get("range")?)?;
                    let new_text = e.get("newText")?.as_str()?.to_string();
                    Some(TextEdit { range, new_text })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(TextDocumentEdit {
        uri,
        version,
        edits,
    })
}

pub(crate) fn parse_position_value(v: &Value) -> Option<Position> {
    Some(Position::new(
        v.get("line")?.as_u64()? as u32,
        v.get("character")?.as_u64()? as u32,
    ))
}

pub(crate) fn parse_range_value(v: &Value) -> Option<Range> {
    Some(Range::new(
        parse_position_value(v.get("start")?)?,
        parse_position_value(v.get("end")?)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_edit_json() -> Value {
        serde_json::json!({
            "textDocument": { "uri": "file:///a.rs", "version": 1 },
            "edits": [{
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } },
                "newText": "bar"
            }]
        })
    }

    #[test]
    fn workspace_edit_keeps_file_operations() {
        let v = serde_json::json!({
            "documentChanges": [
                { "kind": "create", "uri": "file:///new.rs", "options": { "ignoreIfExists": true } },
                { "kind": "rename", "oldUri": "file:///a.rs", "newUri": "file:///b.rs" },
                { "kind": "delete", "uri": "file:///old.rs" },
                text_edit_json(),
            ]
        });
        let we = workspace_edit_from_value(&v).unwrap();
        assert_eq!(we.file_operations.len(), 3);
        assert_eq!(we.document_changes.len(), 1);
        assert_eq!(we.total_edits(), 4);
        match &we.file_operations[0] {
            FileOperation::Create(c) => {
                assert_eq!(c.uri, "file:///new.rs");
                assert_eq!(c.options.as_ref().unwrap().ignore_if_exists, Some(true));
            }
            other => panic!("expected Create, got {other:?}"),
        }
        assert!(
            matches!(&we.file_operations[1], FileOperation::Rename(r) if r.old_uri == "file:///a.rs")
        );
        assert!(
            matches!(&we.file_operations[2], FileOperation::Delete(d) if d.uri == "file:///old.rs")
        );
    }

    #[test]
    fn workspace_edit_parses_changes_map() {
        let v = serde_json::json!({
            "changes": {
                "file:///a.rs": [{
                    "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } },
                    "newText": "bar"
                }]
            }
        });
        let we = workspace_edit_from_value(&v).unwrap();
        assert_eq!(we.total_edits(), 1);
        assert_eq!(we.document_changes[0].uri, "file:///a.rs");
        assert_eq!(we.document_changes[0].version, None);
        assert!(we.file_operations.is_empty());
    }

    #[test]
    fn workspace_edit_rejects_unknown_document_change_entry() {
        let v = serde_json::json!({
            "documentChanges": [{ "kind": "explode", "uri": "file:///x.rs" }]
        });
        assert!(workspace_edit_from_value(&v).is_err());
    }

    #[test]
    fn workspace_edit_serde_round_trips_wire_form() {
        let v = serde_json::json!({
            "documentChanges": [
                { "kind": "delete", "uri": "file:///old.rs" },
                text_edit_json(),
            ]
        });
        let we: WorkspaceEdit = serde_json::from_value(v.clone()).unwrap();
        let wire = serde_json::to_value(&we).unwrap();
        assert_eq!(wire["documentChanges"][0]["kind"].as_str(), Some("delete"));
        assert_eq!(
            wire["documentChanges"][1]["textDocument"]["uri"].as_str(),
            Some("file:///a.rs")
        );
        let back: WorkspaceEdit = serde_json::from_value(wire).unwrap();
        assert_eq!(back, we);
    }
}

/// code action 规范化结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeAction {
    pub title: String,
    pub kind: Option<String>,
    /// 是否带可应用编辑；`None` 表示该 action 不产生编辑。
    #[serde(default)]
    pub edit: Option<WorkspaceEdit>,
    /// 是否为首选 / 快速修复等标记。
    #[serde(default)]
    pub is_preferred: bool,
}

/// 统一接口结果：大体积内容经 artifact 引用，否则内联。
#[derive(Debug, Clone)]
pub enum ResultPayload<T> {
    Inline(T),
    /// 大体积结果已归一为 artifact 引用（调用方可再经 artifact store 读取完整内容）。
    Artifact(ArtifactRef),
}

/// artifact 引用（ADR-018 形态：kind + 稳定 id + 体积）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub store: &'static str,
    pub kind: String,
    pub id: String,
    pub size: u64,
}
