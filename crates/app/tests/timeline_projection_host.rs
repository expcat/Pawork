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

fn load_fixture_events(name: &str) -> (Vec<AgentEventEnvelope>, Vec<TimelineItem>) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../protocol/tests/fixtures/projection")
        .join(name);
    let raw = std::fs::read_to_string(path).expect("read projection fixture");
    let mut envelopes = Vec::new();
    let mut items = Vec::new();
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let value: serde_json::Value = serde_json::from_str(line).expect("parse fixture line");
        envelopes
            .push(serde_json::from_value(value["domain"].clone()).expect("decode domain envelope"));
        if !value["item"].is_null() {
            items
                .push(serde_json::from_value(value["item"].clone()).expect("decode expected item"));
        }
    }
    (envelopes, items)
}

fn decode_event(value: serde_json::Value) -> AgentEventEnvelope {
    serde_json::from_value(value).expect("decode domain event")
}

#[tokio::test]
async fn host_timeline_matches_projection_golden_items() {
    for fixture in ["paged_interleave.jsonl", "thinking.jsonl"] {
        let (envelopes, expected) = load_fixture_events(fixture);
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
        // 真实 Host 查询的旧版本输出不携带新字段，游标仍按持久事件前进。
        for minor in [13, 14] {
            let response = adapter
                .query(&pawork_protocol::AppQueryEnvelope {
                    api_version: pawork_protocol::ApiVersion::new(1, minor),
                    request_id: pawork_domain::QueryId::from("timeline-version"),
                    source: pawork_protocol::CommandSource::Automation,
                    identity: pawork_protocol::ActorIdentity::System,
                    issued_at: Timestamp::from_unix_millis(1),
                    query: pawork_protocol::AppQuery::SessionGet {
                        session_id: session.clone(),
                        timeline_after_sequence: None,
                        timeline_limit: Some(1),
                    },
                })
                .await
                .expect("versioned query");
            let pawork_protocol::AppResponse::Data(data) = response else {
                panic!("data response");
            };
            let wire = &data["timeline_page"];
            assert_eq!(wire["next_sequence"], 1);
            assert_eq!(wire["head_sequence"], page.head_sequence);
            assert_eq!(wire["complete"], false);
            if minor < 14 {
                for item in wire["items"].as_array().unwrap() {
                    assert_ne!(item["kind"], "thinking_delta");
                    assert!(
                        item.get("message_id").is_none() && item.get("thinking_text").is_none()
                    );
                }
                if fixture == "thinking.jsonl" {
                    assert_eq!(wire["items"], serde_json::json!([]));
                }
            } else if fixture == "thinking.jsonl" {
                assert_eq!(wire["items"][0]["kind"], "thinking_delta");
            }
        }
    }
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
