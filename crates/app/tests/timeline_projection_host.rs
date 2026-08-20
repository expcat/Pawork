//! host timeline() 对拍 golden（R3 波 C）：storage 灌入 fixture 事件 →
//! 真实 GuiHostAdapter.timeline() 分页 → 与 protocol 投影 golden 的 item
//! 期望逐条相等，钉死 host 分页路径与 reducer 历史臂同源。

use std::sync::Arc;

use pawork_app::gui_server::GuiHost;
use pawork_app::GuiHostAdapter;
use pawork_domain::{AgentEventEnvelope, SessionId, Timestamp};
use pawork_protocol::TimelineItem;
use pawork_storage::session::SessionStore;
use pawork_testkit::{MockProvider, MockScript};

fn load_fixture_events() -> (Vec<AgentEventEnvelope>, Vec<TimelineItem>) {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../protocol/tests/fixtures/projection/paged_interleave.jsonl"
    );
    let raw = std::fs::read_to_string(path).expect("read projection fixture");
    let mut envelopes = Vec::new();
    let mut items = Vec::new();
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let value: serde_json::Value = serde_json::from_str(line).expect("parse fixture line");
        envelopes.push(
            serde_json::from_value(value["domain"].clone())
                .expect("decode domain envelope"),
        );
        if !value["item"].is_null() {
            items.push(
                serde_json::from_value(value["item"].clone()).expect("decode expected item"),
            );
        }
    }
    (envelopes, items)
}

fn decode_event(value: serde_json::Value) -> AgentEventEnvelope {
    serde_json::from_value(value).expect("decode domain event")
}

#[tokio::test]
async fn host_timeline_matches_projection_golden_items() {
    let (envelopes, expected) = load_fixture_events();
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let session = SessionId::from("s-1");
    store
        .create_session(&session, "golden", Timestamp::from_unix_millis(1))
        .await
        .expect("create session");
    let branch = store
        .get_session(&session)
        .await
        .expect("get session")
        .active_branch;
    for envelope in &envelopes {
        store
            .append_event(&branch, envelope.clone())
            .await
            .expect("append fixture event");
    }

    let provider = MockProvider::sequence(vec![MockScript::new().text("unused").complete()]);
    let core = pawork_app::AppCore::from_parts(
        Arc::new(provider),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let page = adapter
        .timeline(&session, None, Some(500))
        .await
        .expect("timeline page");
    assert!(page.complete);
    assert_eq!(
        page.items, expected,
        "host timeline() must match the projection golden item expectations"
    );
}

#[tokio::test]
async fn host_timeline_cursor_advances_across_unprojected_events() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let session = SessionId::from("s-filtered-page");
    store
        .create_session(&session, "filtered page", Timestamp::from_unix_millis(1))
        .await
        .expect("create session");
    let branch = store
        .get_session(&session)
        .await
        .expect("get session")
        .active_branch;
    let events = [
        decode_event(serde_json::json!({
            "schema_version": 1,
            "session_id": "s-filtered-page",
            "run_id": "r-1",
            "sequence": 1,
            "timestamp": 1001,
            "payload": {
                "type": "context_prepared",
                "data": { "message_count": 1, "estimated_input_tokens": 2 }
            },
            "event_id": "evt-1"
        })),
        decode_event(serde_json::json!({
            "schema_version": 1,
            "session_id": "s-filtered-page",
            "run_id": "r-1",
            "sequence": 2,
            "timestamp": 1002,
            "payload": {
                "type": "run_started",
                "data": { "trigger_message_id": "m-1" }
            },
            "event_id": "evt-2"
        })),
    ];
    for envelope in events {
        store
            .append_event(&branch, envelope)
            .await
            .expect("append event");
    }

    let provider = MockProvider::sequence(vec![MockScript::new().text("unused").complete()]);
    let core = pawork_app::AppCore::from_parts(
        Arc::new(provider),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    let adapter = GuiHostAdapter::new(Arc::new(core));

    // limit=0 必须收敛为最小窗口 1；第一条虽不可投影，仍推进底层游标。
    let first = adapter
        .timeline(&session, None, Some(0))
        .await
        .expect("first page");
    assert!(first.items.is_empty());
    assert!(!first.complete);
    assert_eq!(first.next_sequence, Some(1));

    let second = adapter
        .timeline(&session, first.next_sequence, Some(1))
        .await
        .expect("second page");
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].sequence, 2);
    assert!(second.complete);
    assert_eq!(second.next_sequence, None);
}
