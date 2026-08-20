//! 投影 golden：同一事件序列 live 臂 vs 历史臂两端对拍（CR08-08 根治）。
//!
//! fixture 每行三视图：`domain`（持久化 AgentEventEnvelope）、`wire`（广播
//! AppEventEnvelope，不广播为 null）、`item`（project_event 期望输出）。
//! 同一序列分别直喂 live 臂 / 经 project_event 喂历史臂，终态必须一致；
//! 期望渲染态 JSON 快照钉死三个难点：分页交错、Lagged→Snapshot、fork 切支。

use std::path::Path;

use pawork_domain::AgentEventEnvelope;
use pawork_protocol::projection::{project_event, TimelineEntryKind, TimelineProjection};
use pawork_protocol::{AppEventEnvelope, GlobalSequence, ResumeDisposition, TimelineItem};

struct FixtureLine {
    domain: AgentEventEnvelope,
    wire: Option<AppEventEnvelope>,
    item: Option<TimelineItem>,
}

fn load_fixture(name: &str) -> Vec<FixtureLine> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/projection")
        .join(name);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read fixture {name}: {error}"));
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let value: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("parse fixture {name} line: {error}"));
            FixtureLine {
                domain: serde_json::from_value(value["domain"].clone())
                    .unwrap_or_else(|error| panic!("decode domain envelope in {name}: {error}")),
                wire: match value["wire"].is_null() {
                    true => None,
                    false => Some(serde_json::from_value(value["wire"].clone())
                        .unwrap_or_else(|error| panic!("decode wire envelope in {name}: {error}"))),
                },
                item: match value["item"].is_null() {
                    true => None,
                    false => Some(serde_json::from_value(value["item"].clone())
                        .unwrap_or_else(|error| panic!("decode timeline item in {name}: {error}"))),
                },
            }
        })
        .collect()
}

fn expected_state(name: &str) -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/projection")
        .join(name);
    serde_json::from_str(
        &std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read expected state {name}: {error}")),
    )
    .expect("parse expected state")
}

fn render_kind(kind: &TimelineEntryKind) -> serde_json::Value {
    match kind {
        TimelineEntryKind::UserMessage { text } => {
            serde_json::json!({ "user_message": { "text": text } })
        }
        TimelineEntryKind::AssistantMessage { text } => {
            serde_json::json!({ "assistant_message": { "text": text } })
        }
        TimelineEntryKind::ToolCall { name, status, detail } => {
            serde_json::json!({ "tool_call": { "name": name, "status": status, "detail": detail } })
        }
        TimelineEntryKind::RunState(text) => serde_json::json!({ "run_state": text }),
        TimelineEntryKind::Error(text) => serde_json::json!({ "error": text }),
    }
}

fn render(projection: &TimelineProjection) -> serde_json::Value {
    serde_json::json!({
        "entries": projection
            .entries
            .iter()
            .map(|entry| serde_json::json!({
                "sequence": entry.sequence,
                "event_id": entry.event_id,
                "kind": render_kind(&entry.kind),
                "timestamp": entry.timestamp,
                "run_id": entry.run_id,
            }))
            .collect::<Vec<_>>(),
    })
}

fn apply_history(projection: &mut TimelineProjection, lines: &[FixtureLine]) {
    for line in lines {
        if let Some(item) = project_event(&line.domain) {
            projection.apply_item(&item);
        }
    }
}

/// fixture 自洽：project_event 输出必须等于行内 item 视图（host 映射钉死）。
fn assert_fixture_items_match_project_event(lines: &[FixtureLine], name: &str) {
    for line in lines {
        let projected = project_event(&line.domain);
        assert_eq!(
            projected, line.item,
            "project_event mismatch in {name} for sequence {}",
            line.domain.sequence.0
        );
    }
}

#[test]
fn golden_paged_interleave_and_live_history_parity() {
    let lines = load_fixture("paged_interleave.jsonl");
    assert_fixture_items_match_project_event(&lines, "paged_interleave");

    // 交错：live run 事件先到，历史页回填 1-7，重叠页 5-9 不得双份。
    let mut projection = TimelineProjection::default();
    for line in lines.iter().rev().take(2) {
        let wire = line.wire.as_ref().expect("run events must broadcast");
        assert!(projection.apply_event(wire));
    }
    apply_history(&mut projection, &lines[..7]);
    apply_history(&mut projection, &lines[4..]);
    assert_eq!(
        render(&projection),
        expected_state("paged_interleave.expected.json"),
        "paged interleave final state"
    );

    // 两端对拍：同一序列 live 直喂 vs project_event→apply_item 终态相等
    // （只对广播事件比较——user/committed 不进 live 流，属设计不对称）。
    let mut live = TimelineProjection::default();
    let mut history = TimelineProjection::default();
    for line in &lines {
        if let Some(wire) = &line.wire {
            live.apply_event(wire);
        }
        if let Some(item) = project_event(&line.domain) {
            if line.wire.is_some() {
                history.apply_item(&item);
            }
        }
    }
    assert_eq!(
        live.iter()
            .map(|entry| (
                entry.sequence,
                entry.kind.clone(),
                entry.timestamp.clone(),
                entry.run_id.clone(),
            ))
            .collect::<Vec<_>>(),
        history.iter()
            .map(|entry| (
                entry.sequence,
                entry.kind.clone(),
                entry.timestamp.clone(),
                entry.run_id.clone(),
            ))
            .collect::<Vec<_>>(),
        "live and history arms must converge (CR08-08)"
    );
}

#[test]
fn golden_lagged_to_snapshot_rebuilds_baseline() {
    let lines = load_fixture("lagged_to_snapshot.jsonl");
    assert_fixture_items_match_project_event(&lines, "lagged_to_snapshot");

    // 断档前基线：历史页 1-4。
    let mut projection = TimelineProjection::default();
    apply_history(&mut projection, &lines[..4]);
    let before = projection.entries.clone();

    // Replay：重放的 live 事件（同 sequence）不换基线、不双份。
    projection.apply_resume_disposition(&ResumeDisposition::Replay {
        from_sequence: GlobalSequence(2),
        through_sequence: GlobalSequence(3),
    });
    for line in &lines[1..3] {
        let wire = line.wire.as_ref().expect("delta events must broadcast");
        assert!(!projection.apply_event(wire), "replay dedups by sequence");
    }
    assert_eq!(projection.entries, before, "replay keeps baseline");

    // Lagged→SnapshotRequired：清基线后全量重分页。
    projection.apply_resume_disposition(&ResumeDisposition::SnapshotRequired {
        earliest_available_sequence: GlobalSequence(5),
    });
    assert!(projection.entries.is_empty(), "snapshot required resets baseline");
    apply_history(&mut projection, &lines);
    assert_eq!(
        render(&projection),
        expected_state("lagged_to_snapshot.expected.json"),
        "lagged-to-snapshot rebuilt state"
    );
}

#[test]
fn golden_fork_branch_switch_rebuilds_by_lineage() {
    let lines = load_fixture("fork_branch_switch.jsonl");
    assert_fixture_items_match_project_event(&lines, "fork_branch_switch");

    // 分支 A 页：共享前缀 1-3 + A 独有 4-5。
    let mut projection = TimelineProjection::default();
    apply_history(&mut projection, &lines[..5]);
    assert_eq!(projection.entries.len(), 3, "branch A baseline");

    // 切支：清基线，按新 lineage 页重建（共享前缀 + B 独有 6-7）。
    projection.reset_baseline();
    let branch_b = [&lines[0], &lines[1], &lines[2], &lines[5], &lines[6]];
    for line in branch_b {
        if let Some(item) = project_event(&line.domain) {
            projection.apply_item(&item);
        }
    }
    assert_eq!(
        render(&projection),
        expected_state("fork_branch_switch.expected.json"),
        "fork branch switch rebuilt state"
    );
}
