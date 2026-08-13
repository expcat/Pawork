//! 九项统一消费接口的定向测试。

mod common;
use common::{
    full_capabilities, inline, route_handler, test_descriptor, MockAction, MockServerSpec,
    MockSpawner,
};

use lsp_runtime::{
    ClientCapabilities, EditOrigin, FileOperation, LanguageClient, LspClient, Position, Range,
    ServerSpawnConfig,
};
use serde_json::json;
use std::sync::Arc;

async fn lc_with(handler: common::MockHandler) -> LanguageClient {
    let spawner = MockSpawner::single(handler).into_shared();
    let client = LspClient::start(
        test_descriptor("rust"),
        spawner,
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    LanguageClient::new(client).with_request_timeout(std::time::Duration::from_secs(2))
}

fn loc(line: u32, c0: u32, line1: u32, c1: u32) -> serde_json::Value {
    json!({
        "uri": "file:///a.rs",
        "range": {
            "start": { "line": line, "character": c0 },
            "end": { "line": line1, "character": c1 }
        }
    })
}

#[tokio::test]
async fn hover_returns_normalized_markup() {
    let hover = json!({
        "contents": { "kind": "markdown", "value": "# Title\nsome hover" }
    });
    let h = route_handler(vec![("textDocument/hover", MockAction::Respond(hover))]);
    let lc = lc_with(h).await;
    let res = inline(
        lc.hover("file:///a.rs", Position::new(0, 1), None)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(res.content, "# Title\nsome hover");
    assert_eq!(res.kind, lsp_runtime::MarkupKind::Markdown);
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn definition_returns_locations() {
    let def = json!([loc(1, 0, 1, 5)]);
    let h = route_handler(vec![("textDocument/definition", MockAction::Respond(def))]);
    let lc = lc_with(h).await;
    let res = inline(
        lc.definition("file:///a.rs", Position::new(0, 1), None)
            .await
            .unwrap(),
    );
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].range.start.line, 1);
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn references_returns_locations() {
    let refs = json!([loc(2, 0, 2, 3), loc(4, 0, 4, 3)]);
    let h = route_handler(vec![("textDocument/references", MockAction::Respond(refs))]);
    let lc = lc_with(h).await;
    let res = inline(
        lc.references("file:///a.rs", Position::new(0, 1), true, None)
            .await
            .unwrap(),
    );
    assert_eq!(res.len(), 2);
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn document_symbols_parse_tree() {
    let syms = json!([{
        "name": "main", "kind": 12,
        "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 10 } },
        "selectionRange": { "start": { "line": 0, "character": 3 }, "end": { "line": 0, "character": 7 } },
        "children": []
    }]);
    let h = route_handler(vec![(
        "textDocument/documentSymbol",
        MockAction::Respond(syms),
    )]);
    let lc = lc_with(h).await;
    let res = inline(lc.document_symbols("file:///a.rs", None).await.unwrap());
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].name, "main");
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn document_symbols_flat_symbol_information_is_normalized() {
    let syms = json!([
        {
            "name": "Foo", "kind": 5,
            "location": loc(0, 0, 0, 3),
            "containerName": "crate"
        },
        {
            "name": "bar", "kind": 12,
            "location": loc(1, 0, 1, 3)
        }
    ]);
    let h = route_handler(vec![(
        "textDocument/documentSymbol",
        MockAction::Respond(syms),
    )]);
    let lc = lc_with(h).await;
    let res = inline(lc.document_symbols("file:///a.rs", None).await.unwrap());
    assert_eq!(res.len(), 2);
    assert_eq!(res[0].name, "Foo");
    assert_eq!(res[0].kind, lsp_runtime::SymbolKind::Class);
    assert_eq!(res[0].range.start.line, 0);
    assert_eq!(res[0].selection_range, res[0].range);
    assert_eq!(res[0].detail.as_deref(), Some("crate"));
    assert!(res[0].children.is_empty());
    assert_eq!(res[1].name, "bar");
    assert_eq!(res[1].kind, lsp_runtime::SymbolKind::Function);
    assert_eq!(res[1].detail, None);
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn rename_workspace_edit_keeps_file_operations() {
    let edit = json!({
        "documentChanges": [
            { "kind": "create", "uri": "file:///new.rs", "options": { "ignoreIfExists": true } },
            { "kind": "rename", "oldUri": "file:///a.rs", "newUri": "file:///b.rs" },
            { "kind": "delete", "uri": "file:///old.rs" },
            {
                "textDocument": { "uri": "file:///b.rs", "version": 1 },
                "edits": [{
                    "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } },
                    "newText": "bar"
                }]
            }
        ]
    });
    let h = route_handler(vec![("textDocument/rename", MockAction::Respond(edit))]);
    let lc = lc_with(h).await;
    let we = inline(
        lc.rename("file:///a.rs", Position::new(0, 0), "bar", None)
            .await
            .unwrap(),
    );
    // 文件操作被显式建模，不静默丢弃。
    assert_eq!(we.file_operations.len(), 3);
    assert_eq!(we.document_changes.len(), 1);
    assert_eq!(we.total_edits(), 4);
    match &we.file_operations[0] {
        FileOperation::Create(c) => {
            assert_eq!(c.uri, "file:///new.rs");
            assert_eq!(c.options.as_ref().unwrap().ignore_if_exists, Some(true));
        }
        other => panic!("expected create, got {other:?}"),
    }
    assert!(matches!(
        &we.file_operations[1],
        FileOperation::Rename(r) if r.old_uri == "file:///a.rs" && r.new_uri == "file:///b.rs"
    ));
    assert!(matches!(
        &we.file_operations[2],
        FileOperation::Delete(d) if d.uri == "file:///old.rs"
    ));
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn rename_workspace_edit_accepts_changes_map() {
    let edit = json!({
        "changes": {
            "file:///a.rs": [{
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } },
                "newText": "bar"
            }]
        }
    });
    let h = route_handler(vec![("textDocument/rename", MockAction::Respond(edit))]);
    let lc = lc_with(h).await;
    let we = inline(
        lc.rename("file:///a.rs", Position::new(0, 0), "bar", None)
            .await
            .unwrap(),
    );
    assert_eq!(we.total_edits(), 1);
    assert_eq!(we.document_changes.len(), 1);
    assert_eq!(we.document_changes[0].uri, "file:///a.rs");
    assert_eq!(we.document_changes[0].version, None);
    assert_eq!(we.document_changes[0].edits[0].new_text, "bar");
    assert!(we.file_operations.is_empty());
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn workspace_symbols_parse() {
    let syms = json!([{
        "name": "Foo", "kind": 5,
        "location": loc(0,0,0,3),
        "containerName": "crate"
    }]);
    let h = route_handler(vec![("workspace/symbol", MockAction::Respond(syms))]);
    let lc = lc_with(h).await;
    let res = inline(lc.workspace_symbols("Foo", None).await.unwrap());
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].container_name.as_deref(), Some("crate"));
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn call_hierarchy_prepare_and_edges() {
    let item = json!([{
        "name": "foo", "kind": 12, "uri": "file:///a.rs",
        "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } },
        "selectionRange": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } }
    }]);
    let edge = json!([{
        "item": {
            "name": "bar", "kind": 12, "uri": "file:///b.rs",
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } },
            "selectionRange": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } }
        },
        "fromRanges": []
    }]);
    let item_clone = item.clone();
    let edge_clone = edge.clone();
    let h = Arc::new(
        move |method: &str, _p: &serde_json::Value, _id: bool| match method {
            "textDocument/prepareCallHierarchy" => MockAction::Respond(item_clone.clone()),
            "callHierarchy/incomingCalls" | "callHierarchy/outgoingCalls" => {
                MockAction::Respond(edge_clone.clone())
            }
            _ => MockAction::Respond(serde_json::Value::Null),
        },
    );
    let lc = lc_with(h).await;
    let prep = inline(
        lc.prepare_call_hierarchy("file:///a.rs", Position::new(0, 0), None)
            .await
            .unwrap(),
    );
    assert_eq!(prep.len(), 1);
    let incoming = inline(lc.incoming_calls(&prep[0], None).await.unwrap());
    assert_eq!(incoming.len(), 1);
    let outgoing = inline(lc.outgoing_calls(&prep[0], None).await.unwrap());
    assert_eq!(outgoing.len(), 1);
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn rename_returns_normalized_workspace_edit() {
    let edit = json!({
        "documentChanges": [{
            "textDocument": { "uri": "file:///a.rs", "version": 1 },
            "edits": [{ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } }, "newText": "bar" }]
        }]
    });
    let h = route_handler(vec![("textDocument/rename", MockAction::Respond(edit))]);
    let lc = lc_with(h).await;
    let we = inline(
        lc.rename("file:///a.rs", Position::new(0, 0), "bar", None)
            .await
            .unwrap(),
    );
    assert_eq!(we.total_edits(), 1);
    assert_eq!(we.document_changes[0].edits[0].new_text, "bar");
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn code_actions_parse() {
    let actions = json!([{
        "title": "Import 'Foo'",
        "kind": "quickfix",
        "isPreferred": true,
        "edit": {
            "documentChanges": [{
                "textDocument": { "uri": "file:///a.rs" },
                "edits": [{ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } }, "newText": "use foo::Foo;\n" }]
            }]
        }
    }]);
    let h = route_handler(vec![(
        "textDocument/codeAction",
        MockAction::Respond(actions),
    )]);
    let lc = lc_with(h).await;
    let res = inline(
        lc.code_actions(
            "file:///a.rs",
            Range::new(Position::new(0, 0), Position::new(0, 3)),
            None,
        )
        .await
        .unwrap(),
    );
    assert_eq!(res.len(), 1);
    assert!(res[0].is_preferred);
    assert_eq!(res[0].edit.as_ref().unwrap().total_edits(), 1);
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn unsupported_method_is_gated_by_capability() {
    // 仅开 hover。
    let mut caps = full_capabilities();
    if let Some(obj) = caps.as_object_mut() {
        obj.insert("definitionProvider".to_string(), json!(false));
    }
    let handler = common::route_handler(vec![]);
    let spawner = MockSpawner::new(vec![MockServerSpec {
        capabilities: caps,
        handler,
        init_delay: None,
    }])
    .into_shared();
    let client = LspClient::start(
        test_descriptor("rust"),
        spawner,
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .unwrap();
    let lc = LanguageClient::new(client).with_request_timeout(std::time::Duration::from_secs(1));
    let err = lc
        .definition("file:///a.rs", Position::new(0, 0), None)
        .await
        .unwrap_err();
    assert!(matches!(err, lsp_runtime::LspError::Unsupported { .. }));
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn diagnostics_filter_by_uri() {
    use lsp_runtime::write_policy::AllowThenApplyPolicy;
    // 仅用于让 EditOrigin 可被引用（避免未用告警）。
    let _ = EditOrigin::Rename;
    let handler = route_handler(vec![]);
    let _ = AllowThenApplyPolicy;
    let lc = lc_with(handler).await;
    let diags = inline(lc.diagnostics("file:///none.rs").await.unwrap());
    assert!(diags.is_empty());
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn large_result_offloads_to_artifact_via_facade() {
    use lsp_runtime::artifact::{InMemorySink, ARTIFACT_INLINE_THRESHOLD};
    use lsp_runtime::ResultPayload;

    // 超过阈值的 workspace_symbols 结果（大体积符号表）。
    let big_name = "x".repeat(ARTIFACT_INLINE_THRESHOLD + 1);
    let syms = json!([{
        "name": big_name, "kind": 5,
        "location": loc(0, 0, 0, 3),
    }]);
    let handler = route_handler(vec![("workspace/symbol", MockAction::Respond(syms))]);
    let client = LspClient::start(
        test_descriptor("rust"),
        MockSpawner::single(handler).into_shared(),
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    let sink = Arc::new(InMemorySink::default());
    let lc = LanguageClient::new(client)
        .with_artifact_sink(sink)
        .with_request_timeout(std::time::Duration::from_secs(2));

    let payload = lc
        .fetch_payload::<Vec<lsp_runtime::WorkspaceSymbol>>(
            "workspace/symbol",
            Some(json!({ "query": "x" })),
            None,
        )
        .await
        .unwrap();
    match payload {
        ResultPayload::Artifact(reference) => {
            assert!(reference.size as usize > ARTIFACT_INLINE_THRESHOLD);
        }
        ResultPayload::Inline(_) => panic!("expected artifact offload for oversized result"),
    }

    // 小结果保持内联。
    let small = route_handler(vec![(
        "workspace/symbol",
        MockAction::Respond(json!([{ "name": "Foo", "kind": 5, "location": loc(0, 0, 0, 3) }])),
    )]);
    let client2 = LspClient::start(
        test_descriptor("rust"),
        MockSpawner::single(small).into_shared(),
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    let lc2 = LanguageClient::new(client2)
        .with_artifact_sink(Arc::new(InMemorySink::default()))
        .with_request_timeout(std::time::Duration::from_secs(2));
    let payload2 = lc2
        .fetch_payload::<Vec<lsp_runtime::WorkspaceSymbol>>(
            "workspace/symbol",
            Some(json!({ "query": "Foo" })),
            None,
        )
        .await
        .unwrap();
    match payload2 {
        ResultPayload::Inline(symbols) => {
            assert_eq!(symbols.len(), 1);
            assert_eq!(symbols[0].name, "Foo");
        }
        ResultPayload::Artifact(_) => panic!("expected inline result"),
    }
    lc.shutdown().await.unwrap();
    lc2.shutdown().await.unwrap();
}

#[tokio::test]
async fn nine_interfaces_fail_closed_without_artifact_sink() {
    // 九项统一接口（+pull_diagnostics）的大结果必须统一走 artifact fail-closed：
    // 未注入 sink 时显式失败，绝不把大 payload 内联回流。
    use lsp_runtime::artifact::ARTIFACT_INLINE_THRESHOLD;

    let big = "x".repeat(ARTIFACT_INLINE_THRESHOLD + 1);
    let r = json!({
        "start": { "line": 0, "character": 0 },
        "end": { "line": 0, "character": 1 }
    });
    let big_loc = json!([{ "uri": format!("file:///{big}"), "range": r.clone() }]);
    let big_sym = json!([{ "name": big.clone(), "kind": 5, "location": loc(0, 0, 0, 3) }]);
    let big_doc_sym = json!([{
        "name": big.clone(), "kind": 12,
        "range": r.clone(), "selectionRange": r.clone()
    }]);
    let big_edit = json!({
        "changes": {
            "file:///a.rs": [{ "range": r.clone(), "newText": big.clone() }]
        }
    });
    let big_action = json!([{ "title": big.clone(), "kind": "quickfix" }]);
    let big_hover = json!({ "contents": { "kind": "markdown", "value": big.clone() } });
    let big_item = json!([{
        "name": big.clone(), "kind": 12, "uri": "file:///a.rs",
        "range": r.clone(), "selectionRange": r.clone()
    }]);
    let big_edge = json!([{
        "item": {
            "name": big.clone(), "kind": 12, "uri": "file:///b.rs",
            "range": r.clone(), "selectionRange": r.clone()
        },
        "fromRanges": []
    }]);
    let big_diag = json!([{ "range": r.clone(), "severity": 1, "message": big }]);
    let handler = route_handler(vec![
        ("textDocument/hover", MockAction::Respond(big_hover)),
        (
            "textDocument/definition",
            MockAction::Respond(big_loc.clone()),
        ),
        ("textDocument/references", MockAction::Respond(big_loc)),
        (
            "textDocument/documentSymbol",
            MockAction::Respond(big_doc_sym),
        ),
        ("workspace/symbol", MockAction::Respond(big_sym)),
        (
            "textDocument/prepareCallHierarchy",
            MockAction::Respond(big_item),
        ),
        (
            "callHierarchy/incomingCalls",
            MockAction::Respond(big_edge.clone()),
        ),
        ("callHierarchy/outgoingCalls", MockAction::Respond(big_edge)),
        ("textDocument/rename", MockAction::Respond(big_edit)),
        ("textDocument/codeAction", MockAction::Respond(big_action)),
        (
            "textDocument/diagnostic",
            MockAction::Respond(json!({ "items": big_diag })),
        ),
    ]);
    let lc = lc_with(handler).await;

    let pos = Position::new(0, 0);
    let item: lsp_runtime::CallHierarchyItem = serde_json::from_value(json!({
        "name": "foo", "kind": 12, "uri": "file:///a.rs",
        "range": r, "selectionRange": r
    }))
    .unwrap();
    macro_rules! assert_fail_closed {
        ($name:expr, $fut:expr) => {
            match $fut.await {
                Ok(_) => panic!("{}: 大结果无 sink 必须 fail-closed，却返回了 Ok", $name),
                Err(err) => assert!(
                    err.to_string().contains("requires an artifact sink"),
                    "{}: 大结果无 sink 必须 fail-closed，got {err:?}",
                    $name
                ),
            }
        };
    }
    assert_fail_closed!("hover", lc.hover("file:///a.rs", pos, None));
    assert_fail_closed!("definition", lc.definition("file:///a.rs", pos, None));
    assert_fail_closed!("references", lc.references("file:///a.rs", pos, true, None));
    assert_fail_closed!(
        "document_symbols",
        lc.document_symbols("file:///a.rs", None)
    );
    assert_fail_closed!("workspace_symbols", lc.workspace_symbols("big", None));
    assert_fail_closed!(
        "prepare_call_hierarchy",
        lc.prepare_call_hierarchy("file:///a.rs", pos, None)
    );
    assert_fail_closed!("incoming_calls", lc.incoming_calls(&item, None));
    assert_fail_closed!("outgoing_calls", lc.outgoing_calls(&item, None));
    assert_fail_closed!("rename", lc.rename("file:///a.rs", pos, "bar", None));
    assert_fail_closed!(
        "code_actions",
        lc.code_actions("file:///a.rs", Range::new(pos, pos), None)
    );
    assert_fail_closed!(
        "pull_diagnostics",
        lc.pull_diagnostics("file:///a.rs", None)
    );
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn large_results_use_artifact_sink_when_configured() {
    // 九项统一接口配置 sink 后：大结果返回 Artifact 引用而非内联值。
    use lsp_runtime::artifact::{InMemorySink, ARTIFACT_INLINE_THRESHOLD};
    use lsp_runtime::ResultPayload;

    let big_name = "x".repeat(ARTIFACT_INLINE_THRESHOLD + 1);
    let big_sym = json!([{ "name": big_name, "kind": 5, "location": loc(0, 0, 0, 3) }]);
    let handler = route_handler(vec![("workspace/symbol", MockAction::Respond(big_sym))]);
    let client = LspClient::start(
        test_descriptor("rust"),
        MockSpawner::single(handler).into_shared(),
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    let lc = LanguageClient::new(client)
        .with_artifact_sink(Arc::new(InMemorySink::default()))
        .with_request_timeout(std::time::Duration::from_secs(2));
    let payload = lc.workspace_symbols("big", None).await.unwrap();
    match payload {
        ResultPayload::Artifact(reference) => {
            assert_eq!(reference.kind, "workspace/symbol");
            assert!(reference.size as usize > ARTIFACT_INLINE_THRESHOLD);
        }
        ResultPayload::Inline(_) => panic!("大结果配置 sink 后必须走 artifact"),
    }
    lc.shutdown().await.unwrap();
}
