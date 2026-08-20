//! 投影 reducer 语义测试（R3 波 C，自 desktop projection.rs 随迁）。
//!
//! 覆盖：assistant 去重合并、分页去重与 committed 替换、live/分页交错、
//! resume 三态基线、run 态文案两端一致（CR08-08）。

use pawork_protocol::projection::{TimelineEntryKind, TimelineProjection};
use pawork_protocol::{AppEventEnvelope, GlobalSequence, ResumeDisposition, TimelineItem, TimelineItemKind};

fn event(sequence: u64, payload: serde_json::Value) -> AppEventEnvelope {
    serde_json::from_value(serde_json::json!({
        "api_version": { "major": 1, "minor": 1 },
        "instance_id": "instance-1",
        "event_id": format!("app-{sequence}"),
        "global_sequence": sequence,
        "stream": { "type": "session", "id": "s-1" },
        "stream_sequence": sequence,
        "timestamp": 1_000 + sequence,
        "source": { "type": "core" },
        "payload": payload
    }))
    .expect("decode AppEventEnvelope")
}

fn run_changed(sequence: u64, state: &str) -> AppEventEnvelope {
    event(
        sequence,
        serde_json::json!({ "type": "run_changed", "data": { "run_id": "r-1", "state": state } }),
    )
}

fn assistant_delta(sequence: u64, message_id: &str, delta: &str) -> AppEventEnvelope {
    event(
        sequence,
        serde_json::json!({
            "type": "assistant_delta",
            "data": { "run_id": "r-1", "message_id": message_id, "delta": delta }
        }),
    )
}

fn tool_started(sequence: u64, tool_call_id: &str, name: &str) -> AppEventEnvelope {
    event(
        sequence,
        serde_json::json!({
            "type": "tool_started",
            "data": { "run_id": "r-1", "tool_call_id": tool_call_id, "name": name }
        }),
    )
}

fn tool_output(sequence: u64, tool_call_id: &str, delta: &str) -> AppEventEnvelope {
    event(
        sequence,
        serde_json::json!({
            "type": "tool_output",
            "data": {
                "run_id": "r-1",
                "tool_call_id": tool_call_id,
                "delta": delta,
                "truncated": false
            }
        }),
    )
}

fn tool_completed(sequence: u64, tool_call_id: &str, success: bool) -> AppEventEnvelope {
    event(
        sequence,
        serde_json::json!({
            "type": "tool_completed",
            "data": { "run_id": "r-1", "tool_call_id": tool_call_id, "success": success }
        }),
    )
}

/// 类型化历史条目（历史臂不再手工解构 JSON）。
fn history_item(sequence: u64, kind: TimelineItemKind) -> TimelineItem {
    TimelineItem {
        sequence,
        event_id: format!("hist-{sequence}"),
        kind,
        run_id: Some("r-1".into()),
        text: None,
        tool_name: None,
        status: None,
        detail: None,
        timestamp: "2000".into(),
    }
}

fn item_with(mut item: TimelineItem, text: Option<&str>, tool_name: Option<&str>, status: Option<&str>, detail: Option<&str>) -> TimelineItem {
    item.text = text.map(str::to_string);
    item.tool_name = tool_name.map(str::to_string);
    item.status = status.map(str::to_string);
    item.detail = detail.map(str::to_string);
    item
}

fn assistant_texts(projection: &TimelineProjection) -> Vec<&str> {
    projection
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            TimelineEntryKind::AssistantMessage { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn assistant_deltas_merge_until_message_or_run_changes() {
    let mut projection = TimelineProjection::default();

    assert!(projection.apply_event(&assistant_delta(1, "m-1", "a")));
    assert!(projection.apply_event(&assistant_delta(2, "m-1", "b")));
    assert!(projection.apply_event(&assistant_delta(3, "m-2", "c")));
    assert_eq!(projection.entries.len(), 2);
    assert_eq!(assistant_texts(&projection), vec!["ab", "c"]);
}

#[test]
fn timeline_items_dedup_by_sequence_and_merge_committed_text() {
    let mut projection = TimelineProjection::default();

    let first = vec![
        item_with(history_item(1, TimelineItemKind::UserMessage), Some("hi"), None, None, None),
        item_with(history_item(2, TimelineItemKind::AssistantDelta), Some("He"), None, None, None),
        item_with(history_item(3, TimelineItemKind::AssistantDelta), Some("llo"), None, None, None),
        item_with(history_item(4, TimelineItemKind::AssistantMessage), Some("Hello"), None, None, None),
    ];
    for item in &first {
        projection.apply_item(item);
    }
    // 重放同页：sequence 去重，条目数不变。
    for item in &first {
        projection.apply_item(item);
    }
    assert_eq!(projection.entries.len(), 2);
    assert!(matches!(
        &projection.entries[1].kind,
        TimelineEntryKind::AssistantMessage { text } if text == "Hello"
    ));
    // committed 替换后条目携带 committed 的 sequence。
    assert_eq!(projection.entries[1].sequence, 4);

    let second = vec![
        item_with(history_item(3, TimelineItemKind::AssistantDelta), Some("llo"), None, None, None),
        item_with(history_item(5, TimelineItemKind::ToolStarted), None, Some("fs_read"), Some("running"), None),
        item_with(history_item(6, TimelineItemKind::ToolOutput), Some("42 bytes"), None, None, None),
        item_with(history_item(7, TimelineItemKind::ToolCompleted), None, Some("fs_read"), Some("succeeded"), None),
    ];
    for item in &second {
        projection.apply_item(item);
    }
    assert_eq!(projection.entries.len(), 3);
    assert!(matches!(
        &projection.entries[2].kind,
        TimelineEntryKind::ToolCall { name, status, detail }
            if name == "fs_read" && status == "succeeded" && detail.as_deref() == Some("42 bytes")
    ));

    // 页数据之外先到的 live 事件重放（同 sequence）不再重复。
    assert!(!projection.apply_event(&assistant_delta(2, "m-1", "He")));
}

#[test]
fn historical_assistant_rounds_in_same_run_remain_distinct() {
    let mut projection = TimelineProjection::default();
    let items = [
        item_with(
            history_item(1, TimelineItemKind::AssistantDelta),
            Some("first "),
            None,
            None,
            None,
        ),
        item_with(
            history_item(2, TimelineItemKind::AssistantMessage),
            Some("first answer"),
            None,
            None,
            None,
        ),
        item_with(
            history_item(3, TimelineItemKind::AssistantDelta),
            Some("second "),
            None,
            None,
            None,
        ),
        item_with(
            history_item(4, TimelineItemKind::AssistantMessage),
            Some("second answer"),
            None,
            None,
            None,
        ),
    ];
    for item in &items {
        projection.apply_item(item);
    }

    assert_eq!(
        assistant_texts(&projection),
        vec!["first answer", "second answer"]
    );
    assert_eq!(
        projection
            .entries
            .iter()
            .map(|entry| entry.sequence)
            .collect::<Vec<_>>(),
        vec![2, 4]
    );
}

#[test]
fn historical_committed_replacement_preserves_sequence_order() {
    let mut projection = TimelineProjection::default();
    projection.apply_item(&item_with(
        history_item(5, TimelineItemKind::AssistantDelta),
        Some("draft"),
        None,
        None,
        None,
    ));
    projection.apply_item(&item_with(
        history_item(7, TimelineItemKind::ToolStarted),
        None,
        Some("fs_read"),
        Some("running"),
        None,
    ));
    projection.apply_item(&item_with(
        history_item(8, TimelineItemKind::AssistantMessage),
        Some("committed"),
        None,
        None,
        None,
    ));

    assert_eq!(
        projection
            .entries
            .iter()
            .map(|entry| entry.sequence)
            .collect::<Vec<_>>(),
        vec![7, 8]
    );
    assert_eq!(assistant_texts(&projection), vec!["committed"]);
}

#[test]
fn historical_committed_reconciles_live_anchor_and_absorbs_late_delta() {
    let mut projection = TimelineProjection::default();

    assert!(projection.apply_event(&assistant_delta(2, "m-1", "draft")));
    projection.apply_item(&item_with(
        history_item(4, TimelineItemKind::AssistantMessage),
        Some("committed"),
        None,
        None,
        None,
    ));

    assert_eq!(assistant_texts(&projection), vec!["committed"]);
    assert_eq!(projection.entries[0].sequence, 4);
    // committed 已是权威全文；后到、但 sequence 更早的同 message delta 不得
    // 再次追加，也不得新开重复 assistant 条目。
    assert!(!projection.apply_event(&assistant_delta(3, "m-1", " late")));
    assert_eq!(assistant_texts(&projection), vec!["committed"]);

    // 新 message 仍能开启下一轮 assistant 输出。
    assert!(projection.apply_event(&assistant_delta(5, "m-2", "next")));
    assert_eq!(assistant_texts(&projection), vec!["committed", "next"]);
}

#[test]
fn older_historical_committed_does_not_clear_newer_live_anchor() {
    let mut projection = TimelineProjection::default();

    assert!(projection.apply_event(&assistant_delta(10, "m-new", "new")));
    projection.apply_item(&item_with(
        history_item(4, TimelineItemKind::AssistantMessage),
        Some("old"),
        None,
        None,
        None,
    ));
    assert!(projection.apply_event(&assistant_delta(11, "m-new", " answer")));

    assert_eq!(assistant_texts(&projection), vec!["old", "new answer"]);
    assert_eq!(
        projection
            .entries
            .iter()
            .map(|entry| entry.sequence)
            .collect::<Vec<_>>(),
        vec![4, 10]
    );
}

#[test]
fn concurrent_historical_tools_keep_outputs_by_tool_call_id() {
    let mut projection = TimelineProjection::default();
    let call_a = r#"{"_pawork_tool_call_id":"call-a"}"#;
    let call_b = r#"{"_pawork_tool_call_id":"call-b"}"#;
    let items = [
        item_with(
            history_item(1, TimelineItemKind::ToolStarted),
            None,
            Some("fs_read"),
            Some("running"),
            Some(call_a),
        ),
        item_with(
            history_item(2, TimelineItemKind::ToolStarted),
            None,
            Some("shell"),
            Some("running"),
            Some(call_b),
        ),
        item_with(
            history_item(3, TimelineItemKind::ToolOutput),
            Some("output-a"),
            None,
            None,
            Some(call_a),
        ),
        item_with(
            history_item(4, TimelineItemKind::ToolOutput),
            Some("output-b"),
            None,
            None,
            Some(call_b),
        ),
        item_with(
            history_item(5, TimelineItemKind::ToolCompleted),
            None,
            Some("fs_read"),
            Some("succeeded"),
            Some(call_a),
        ),
        item_with(
            history_item(6, TimelineItemKind::ToolCompleted),
            None,
            Some("shell"),
            Some("failed"),
            Some(call_b),
        ),
    ];
    for item in &items {
        projection.apply_item(item);
    }

    let tools = projection
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            TimelineEntryKind::ToolCall {
                name,
                status,
                detail,
            } => Some((name.as_str(), status.as_str(), detail.as_deref())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tools,
        vec![
            ("fs_read", "succeeded", Some("output-a")),
            ("shell", "failed", Some("output-b")),
        ]
    );
}

#[test]
fn duplicate_live_start_enriches_legacy_history_tool_anchor() {
    let mut projection = TimelineProjection::default();
    projection.apply_item(&item_with(
        history_item(10, TimelineItemKind::ToolStarted),
        None,
        Some("fs_read"),
        Some("running"),
        None,
    ));

    assert!(!projection.apply_event(&tool_started(10, "call-1", "fs_read")));
    assert!(projection.apply_event(&tool_output(11, "call-1", "42 bytes")));
    assert!(projection.apply_event(&tool_completed(12, "call-1", true)));

    assert_eq!(projection.entries.len(), 1);
    assert!(matches!(
        &projection.entries[0].kind,
        TimelineEntryKind::ToolCall { name, status, detail }
            if name == "fs_read"
                && status == "succeeded"
                && detail.as_deref() == Some("42 bytes")
    ));
}

#[test]
fn duplicate_live_tool_output_is_not_appended_twice() {
    let mut projection = TimelineProjection::default();
    assert!(projection.apply_event(&tool_started(1, "call-1", "fs_read")));
    let output = tool_output(2, "call-1", "chunk");
    assert!(projection.apply_event(&output));
    assert!(!projection.apply_event(&output));

    assert!(matches!(
        &projection.entries[0].kind,
        TimelineEntryKind::ToolCall { detail, .. }
            if detail.as_deref() == Some("chunk")
    ));
}

#[test]
fn live_tool_survives_earlier_page_insert_without_duplicate() {
    let mut projection = TimelineProjection::default();

    assert!(projection.apply_event(&tool_started(10, "call-1", "fs_read")));
    assert_eq!(projection.entries.len(), 1);
    assert!(matches!(
        &projection.entries[0].kind,
        TimelineEntryKind::ToolCall { name, status, .. }
            if name == "fs_read" && status == "running"
    ));

    projection.apply_item(&item_with(history_item(5, TimelineItemKind::UserMessage), Some("hi"), None, None, None));
    assert_eq!(projection.entries.len(), 2);
    assert!(matches!(
        &projection.entries[0].kind,
        TimelineEntryKind::UserMessage { text } if text == "hi"
    ));

    assert!(projection.apply_event(&tool_completed(11, "call-1", true)));
    let tools: Vec<(&str, &str)> = projection
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            TimelineEntryKind::ToolCall { name, status, .. } => {
                Some((name.as_str(), status.as_str()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(tools, vec![("fs_read", "succeeded")]);
}

#[test]
fn live_assistant_survives_earlier_page_insert_without_split() {
    let mut projection = TimelineProjection::default();

    assert!(projection.apply_event(&assistant_delta(10, "m-1", "Hello")));
    projection.apply_item(&item_with(history_item(5, TimelineItemKind::UserMessage), Some("hi"), None, None, None));
    assert!(projection.apply_event(&assistant_delta(11, "m-1", " world")));

    assert_eq!(assistant_texts(&projection), vec!["Hello world"]);
    assert!(matches!(
        &projection.entries[0].kind,
        TimelineEntryKind::UserMessage { text } if text == "hi"
    ));
}

#[test]
fn replay_continues_timeline_without_replacing_baseline() {
    let mut projection = TimelineProjection::default();
    assert!(projection.apply_event(&assistant_delta(2, "m-1", "Hello")));
    assert_eq!(projection.entries.len(), 1);

    projection.apply_resume_disposition(&ResumeDisposition::Replay {
        from_sequence: GlobalSequence(3),
        through_sequence: GlobalSequence(4),
    });
    assert!(projection.apply_event(&assistant_delta(3, "m-1", " ")));
    assert!(projection.apply_event(&assistant_delta(4, "m-1", "world")));
    assert_eq!(projection.entries.len(), 1);
    assert_eq!(assistant_texts(&projection), vec!["Hello world"]);
    // 同 sequence 再来一遍不得双份。
    assert!(!projection.apply_event(&assistant_delta(3, "m-1", " ")));
}

#[test]
fn snapshot_required_discards_stale_baseline() {
    let mut projection = TimelineProjection::default();
    assert!(projection.apply_event(&assistant_delta(1, "m-1", "stale")));
    assert_eq!(projection.entries.len(), 1);

    projection.apply_resume_disposition(&ResumeDisposition::SnapshotRequired {
        earliest_available_sequence: GlobalSequence(8),
    });
    assert_eq!(projection.entries.len(), 0);
    // 基线清空后同 sequence 重建不误判重复。
    assert!(projection.apply_event(&assistant_delta(1, "m-1", "fresh")));
}

#[test]
fn up_to_date_keeps_baseline() {
    let mut projection = TimelineProjection::default();
    assert!(projection.apply_event(&assistant_delta(1, "m-1", "keep")));
    projection.apply_resume_disposition(&ResumeDisposition::UpToDate {
        current_sequence: GlobalSequence(1),
    });
    assert_eq!(projection.entries.len(), 1);
    assert_eq!(assistant_texts(&projection), vec!["keep"]);
}

#[test]
fn run_state_labels_agree_between_live_and_history_arms() {
    // CR08-08 根治点：同一 run 生命周期 live 与历史臂文案一致。
    for (state, kind, label) in [
        ("created", TimelineItemKind::RunStarted, "run started"),
        ("completed", TimelineItemKind::RunCompleted, "run completed"),
        ("cancelled", TimelineItemKind::RunCancelled, "run cancelled"),
        ("failed", TimelineItemKind::RunFailed, "run failed"),
    ] {
        let mut live = TimelineProjection::default();
        assert!(live.apply_event(&run_changed(1, state)));
        let mut history = TimelineProjection::default();
        history.apply_item(&history_item(1, kind));
        let live_label = match &live.entries[0].kind {
            TimelineEntryKind::RunState(text) => text.clone(),
            other => panic!("live arm should render run state, got {other:?}"),
        };
        let history_label = match &history.entries[0].kind {
            TimelineEntryKind::RunState(text) => text.clone(),
            other => panic!("history arm should render run state, got {other:?}"),
        };
        assert_eq!(live_label, history_label);
        assert_eq!(live_label, label);
    }
}

#[test]
fn fifty_thousand_timeline_entries_iter_without_clone() {
    let mut projection = TimelineProjection::default();
    projection.entries.reserve(50_000);
    for sequence in 0..50_000u64 {
        projection.entries.push(pawork_protocol::projection::TimelineEntry {
            sequence,
            event_id: format!("e{sequence}"),
            kind: TimelineEntryKind::RunState("x".into()),
            timestamp: "1".into(),
            run_id: None,
        });
    }
    let started = std::time::Instant::now();
    let count = projection.entries.iter().map(|entry| entry.sequence).count();
    let elapsed = started.elapsed();
    assert_eq!(count, 50_000);
    assert!(
        elapsed.as_millis() < 100,
        "borrowed timeline iter should stay cheap, took {elapsed:?}"
    );
}
