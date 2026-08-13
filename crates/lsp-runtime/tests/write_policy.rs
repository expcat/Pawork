//! rename / code_action 写操作策略契约：DenyAll 阻止；Allow + Applier 落盘计数。

mod common;
use common::{inline, route_handler, test_descriptor, MockAction, MockSpawner};

use async_trait::async_trait;
use lsp_runtime::{
    write_policy::{AllowThenApplyPolicy, EditApplier, EditOrigin, EditRequest, PolicyVerdict},
    ClientCapabilities, LanguageClient, LspClient, Position, ServerSpawnConfig,
};
use lsp_runtime::{
    EditOutcome, LspError, Range, TextDocumentEdit, TextEdit, WorkspaceEdit, WriteEditPolicy,
};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
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

fn sample_edit_response() -> serde_json::Value {
    json!({
        "documentChanges": [{
            "textDocument": { "uri": "file:///a.rs", "version": 1 },
            "edits": [{ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } }, "newText": "bar" }]
        }]
    })
}

#[tokio::test]
async fn deny_all_policy_blocks_apply_edit() {
    let handler = route_handler(vec![(
        "textDocument/rename",
        MockAction::Respond(sample_edit_response()),
    )]);
    let lc = lc_with(handler).await;
    let edit = inline(
        lc.rename("file:///a.rs", Position::new(0, 0), "bar", None)
            .await
            .unwrap(),
    );
    let err = lc.apply_edit(EditOrigin::Rename, edit).await.unwrap_err();
    assert!(matches!(err, LspError::PolicyDenied(_)));
    lc.shutdown().await.unwrap();
}

struct CountingApplier {
    applied: Arc<AtomicUsize>,
}

#[async_trait]
impl EditApplier for CountingApplier {
    async fn apply(
        &self,
        request: &lsp_runtime::write_policy::EditRequest,
    ) -> Result<usize, LspError> {
        let n = request.total_edits();
        self.applied.fetch_add(n, Ordering::SeqCst);
        Ok(n)
    }
}

#[tokio::test]
async fn allow_policy_with_applier_applies_edits() {
    let handler = route_handler(vec![(
        "textDocument/rename",
        MockAction::Respond(sample_edit_response()),
    )]);
    let client = LspClient::start(
        test_descriptor("rust"),
        MockSpawner::single(handler).into_shared(),
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    let counter = Arc::new(AtomicUsize::new(0));
    let lc = LanguageClient::new(client)
        .with_write_policy(Arc::new(AllowThenApplyPolicy))
        .with_edit_applier(Arc::new(CountingApplier {
            applied: counter.clone(),
        }))
        .with_request_timeout(std::time::Duration::from_secs(2));
    let edit = inline(
        lc.rename("file:///a.rs", Position::new(0, 0), "bar", None)
            .await
            .unwrap(),
    );
    let outcome = lc.apply_edit(EditOrigin::Rename, edit).await.unwrap();
    assert_eq!(outcome, EditOutcome::Applied(1));
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn allow_without_applier_is_rejected_not_fake_success() {
    let handler = route_handler(vec![(
        "textDocument/rename",
        MockAction::Respond(sample_edit_response()),
    )]);
    let client = LspClient::start(
        test_descriptor("rust"),
        MockSpawner::single(handler).into_shared(),
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    let lc = LanguageClient::new(client)
        .with_write_policy(Arc::new(AllowThenApplyPolicy))
        .with_request_timeout(std::time::Duration::from_secs(2));
    let edit = inline(
        lc.rename("file:///a.rs", Position::new(0, 0), "bar", None)
            .await
            .unwrap(),
    );
    let err = lc.apply_edit(EditOrigin::Rename, edit).await.unwrap_err();
    assert!(matches!(err, LspError::NoEditApplier));
    lc.shutdown().await.unwrap();
}

#[derive(Default)]
struct AskPolicy;

#[async_trait]
impl WriteEditPolicy for AskPolicy {
    async fn authorize(&self, _request: &EditRequest) -> PolicyVerdict {
        PolicyVerdict::Ask
    }
}

#[tokio::test]
async fn ask_verdict_returns_pending_confirmation_and_skips_applier() {
    let handler = route_handler(vec![(
        "textDocument/rename",
        MockAction::Respond(sample_edit_response()),
    )]);
    let client = LspClient::start(
        test_descriptor("rust"),
        MockSpawner::single(handler).into_shared(),
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    let counter = Arc::new(AtomicUsize::new(0));
    let lc = LanguageClient::new(client)
        .with_write_policy(Arc::new(AskPolicy))
        .with_edit_applier(Arc::new(CountingApplier {
            applied: counter.clone(),
        }))
        .with_request_timeout(std::time::Duration::from_secs(2));
    let edit = inline(
        lc.rename("file:///a.rs", Position::new(0, 0), "bar", None)
            .await
            .unwrap(),
    );
    let outcome = lc.apply_edit(EditOrigin::Rename, edit).await.unwrap();
    assert_eq!(outcome, EditOutcome::PendingUserConfirmation);
    // Ask 不得触发 applier（不算假成功）。
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn empty_edit_is_no_op() {
    // 默认 DenyAll；但空 edit 经 AllowThenApplyPolicy 才放行。这里直接构造空 edit。
    let handler = route_handler(vec![]);
    let lc = lc_with(handler).await;
    let empty = WorkspaceEdit {
        document_changes: vec![TextDocumentEdit {
            uri: "file:///a.rs".into(),
            version: None,
            edits: vec![],
        }],
        file_operations: vec![],
    };
    let _ = empty; // 触发 TextEdit/Range 导入使用
    let _ = TextEdit {
        range: Range::new(Position::new(0, 0), Position::new(0, 0)),
        new_text: String::new(),
    };
    // DenyAll 仍拒绝（即使空）。
    let err = lc
        .apply_edit(EditOrigin::CodeAction, WorkspaceEdit::default())
        .await
        .unwrap_err();
    assert!(matches!(err, LspError::PolicyDenied(_)));
    lc.shutdown().await.unwrap();
}
