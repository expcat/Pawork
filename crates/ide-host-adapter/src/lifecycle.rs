//! IDE 适配 trait 与编辑器生命周期映射。
//!
//! 把 IDE 侧差异（VS Code / JetBrains 等）归一为 [`EditorLifecycleEvent`]，
//! 经 [`IdeLifecycle`] 翻译为扩展契约帧并维护 [`EditorContext`]（活动文档、
//! 选区、可见范围、保存版本）。IDE 不直接构造 Core——生命周期上下文只作为
//! Adapter 侧会话上下文，Core 可消费的会话/run 操作仍经 SDK/Headless 通道。

use std::collections::BTreeMap;

use core_api::{ClientDocumentContext, ClientTextPosition, ClientTextRange};
use lsp_runtime::Range;

use crate::contract::{IdeEvent, IdeRequest};

/// 编辑器生命周期事件（IDE 侧差异的 canonical 形态）。
#[derive(Clone, Debug, PartialEq)]
pub enum EditorLifecycleEvent {
    DocumentOpened {
        uri: String,
        language_id: String,
        text: Option<String>,
    },
    DocumentClosed {
        uri: String,
    },
    DocumentActivated {
        uri: String,
    },
    SelectionChanged {
        uri: String,
        selection: Range,
    },
    VisibleRangeChanged {
        uri: String,
        range: Range,
    },
    DocumentSaved {
        uri: String,
    },
}

/// IDE 适配 trait：把编辑器生命周期事件翻译为契约帧并应用到上下文。
pub trait IdeLifecycle: Send + Sync {
    /// 翻译为扩展契约请求；`None` 表示该事件无对应契约帧。
    fn translate(&self, event: &EditorLifecycleEvent) -> Option<IdeRequest>;

    /// 应用到上下文状态（不产生契约帧的副作用）。
    fn apply(&self, event: &EditorLifecycleEvent, context: &mut EditorContext);
}

/// 默认生命周期映射器：逐事件 1:1 翻译 + 上下文维护。
#[derive(Clone, Copy, Debug, Default)]
pub struct LifecycleMapper;

impl IdeLifecycle for LifecycleMapper {
    fn translate(&self, event: &EditorLifecycleEvent) -> Option<IdeRequest> {
        Some(match event {
            EditorLifecycleEvent::DocumentOpened {
                uri,
                language_id,
                text,
            } => IdeRequest::EditorDidOpen {
                document_uri: uri.clone(),
                language_id: language_id.clone(),
                text: text.clone(),
            },
            EditorLifecycleEvent::DocumentClosed { uri } => IdeRequest::EditorDidClose {
                document_uri: uri.clone(),
            },
            EditorLifecycleEvent::DocumentActivated { uri } => IdeRequest::EditorDidActivate {
                document_uri: uri.clone(),
            },
            EditorLifecycleEvent::SelectionChanged { uri, selection } => {
                IdeRequest::EditorDidChangeSelection {
                    document_uri: uri.clone(),
                    selection: *selection,
                }
            }
            EditorLifecycleEvent::VisibleRangeChanged { uri, range } => {
                IdeRequest::EditorDidChangeVisibleRange {
                    document_uri: uri.clone(),
                    range: *range,
                }
            }
            EditorLifecycleEvent::DocumentSaved { uri } => IdeRequest::EditorDidSave {
                document_uri: uri.clone(),
            },
        })
    }

    fn apply(&self, event: &EditorLifecycleEvent, context: &mut EditorContext) {
        context.apply(event);
    }
}

/// 单个文档的编辑器状态（Adapter 侧，非权威）。
#[derive(Clone, Debug, PartialEq)]
pub struct DocumentState {
    pub language_id: String,
    pub selection: Option<Range>,
    pub visible_range: Option<Range>,
    pub saved_version: u64,
    pub text_bytes: Option<usize>,
}

/// Adapter 侧编辑器上下文：活动文档、打开文档集合与选区。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditorContext {
    active_uri: Option<String>,
    documents: BTreeMap<String, DocumentState>,
}

impl EditorContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, event: &EditorLifecycleEvent) {
        match event {
            EditorLifecycleEvent::DocumentOpened {
                uri,
                language_id,
                text,
            } => {
                self.documents.insert(
                    uri.clone(),
                    DocumentState {
                        language_id: language_id.clone(),
                        selection: None,
                        visible_range: None,
                        saved_version: 0,
                        text_bytes: text.as_ref().map(|text| text.len()),
                    },
                );
                // 打开即激活（IDE 语义：打开文档会带到前台）。
                self.active_uri = Some(uri.clone());
            }
            EditorLifecycleEvent::DocumentClosed { uri } => {
                self.documents.remove(uri);
                if self.active_uri.as_deref() == Some(uri.as_str()) {
                    self.active_uri = None;
                }
            }
            EditorLifecycleEvent::DocumentActivated { uri } => {
                if self.documents.contains_key(uri) {
                    self.active_uri = Some(uri.clone());
                }
            }
            EditorLifecycleEvent::SelectionChanged { uri, selection } => {
                if let Some(document) = self.documents.get_mut(uri) {
                    document.selection = Some(*selection);
                }
            }
            EditorLifecycleEvent::VisibleRangeChanged { uri, range } => {
                if let Some(document) = self.documents.get_mut(uri) {
                    document.visible_range = Some(*range);
                }
            }
            EditorLifecycleEvent::DocumentSaved { uri } => {
                if let Some(document) = self.documents.get_mut(uri) {
                    document.saved_version += 1;
                }
            }
        }
    }

    pub fn active_uri(&self) -> Option<&str> {
        self.active_uri.as_deref()
    }

    pub fn active_selection(&self) -> Option<Range> {
        self.active_uri
            .as_ref()
            .and_then(|uri| self.documents.get(uri))
            .and_then(|document| document.selection)
    }

    pub fn document(&self, uri: &str) -> Option<&DocumentState> {
        self.documents.get(uri)
    }

    pub fn open_documents(&self) -> Vec<String> {
        self.documents.keys().cloned().collect()
    }

    /// 导出给 Core 的有限元数据快照；不包含文档正文，`text_bytes` 仅用于
    /// 上下文规模提示，真实文件读取仍必须经 Workspace/Policy。
    pub fn client_documents(&self) -> Vec<ClientDocumentContext> {
        self.documents
            .iter()
            .map(|(uri, document)| ClientDocumentContext {
                uri: uri.clone(),
                language_id: document.language_id.clone(),
                selection: document.selection.map(client_range),
                visible_range: document.visible_range.map(client_range),
                saved_version: document.saved_version,
                text_bytes: document
                    .text_bytes
                    .and_then(|value| u64::try_from(value).ok()),
            })
            .collect()
    }

    /// 上下文变化事件（生命周期帧的契约输出）。
    pub fn context_changed_event(&self) -> IdeEvent {
        IdeEvent::EditorContextChanged {
            active_uri: self.active_uri.clone(),
            selection: self.active_selection(),
            open_documents: self.open_documents(),
        }
    }
}

pub(crate) fn client_range(range: Range) -> ClientTextRange {
    ClientTextRange {
        start: ClientTextPosition {
            line: range.start.line,
            character: range.start.character,
        },
        end: ClientTextPosition {
            line: range.end.line,
            character: range.end.character,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::IdeRequest;
    use lsp_runtime::{Position, Range};

    fn range() -> Range {
        Range::new(Position::new(1, 2), Position::new(3, 4))
    }

    #[test]
    fn mapper_translates_every_lifecycle_event() {
        let mapper = LifecycleMapper;
        assert_eq!(
            mapper.translate(&EditorLifecycleEvent::DocumentOpened {
                uri: "file:///a.rs".into(),
                language_id: "rust".into(),
                text: Some("fn main() {}".into()),
            }),
            Some(IdeRequest::EditorDidOpen {
                document_uri: "file:///a.rs".into(),
                language_id: "rust".into(),
                text: Some("fn main() {}".into()),
            })
        );
        assert_eq!(
            mapper.translate(&EditorLifecycleEvent::SelectionChanged {
                uri: "file:///a.rs".into(),
                selection: range(),
            }),
            Some(IdeRequest::EditorDidChangeSelection {
                document_uri: "file:///a.rs".into(),
                selection: range(),
            })
        );
        assert_eq!(
            mapper.translate(&EditorLifecycleEvent::DocumentSaved {
                uri: "file:///a.rs".into(),
            }),
            Some(IdeRequest::EditorDidSave {
                document_uri: "file:///a.rs".into(),
            })
        );
    }

    #[test]
    fn context_tracks_documents_and_selection() {
        let mut context = EditorContext::new();
        let mapper = LifecycleMapper;

        mapper.apply(
            &EditorLifecycleEvent::DocumentOpened {
                uri: "file:///a.rs".into(),
                language_id: "rust".into(),
                text: Some("code".into()),
            },
            &mut context,
        );
        mapper.apply(
            &EditorLifecycleEvent::DocumentActivated {
                uri: "file:///a.rs".into(),
            },
            &mut context,
        );
        mapper.apply(
            &EditorLifecycleEvent::SelectionChanged {
                uri: "file:///a.rs".into(),
                selection: range(),
            },
            &mut context,
        );
        mapper.apply(
            &EditorLifecycleEvent::DocumentSaved {
                uri: "file:///a.rs".into(),
            },
            &mut context,
        );

        assert_eq!(context.active_uri(), Some("file:///a.rs"));
        assert_eq!(context.active_selection(), Some(range()));
        assert_eq!(context.document("file:///a.rs").unwrap().saved_version, 1);
        assert_eq!(context.open_documents(), vec!["file:///a.rs".to_string()]);

        let event = context.context_changed_event();
        match event {
            IdeEvent::EditorContextChanged {
                active_uri,
                selection,
                open_documents,
            } => {
                assert_eq!(active_uri.as_deref(), Some("file:///a.rs"));
                assert_eq!(selection, Some(range()));
                assert_eq!(open_documents, vec!["file:///a.rs".to_string()]);
            }
            other => panic!("unexpected event: {other:?}"),
        }

        mapper.apply(
            &EditorLifecycleEvent::DocumentClosed {
                uri: "file:///a.rs".into(),
            },
            &mut context,
        );
        assert_eq!(context.active_uri(), None);
        assert!(context.open_documents().is_empty());
    }
}
