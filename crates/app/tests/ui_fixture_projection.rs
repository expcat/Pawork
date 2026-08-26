//! R1 Wave B Phase C：UI fixture 投影断言（seed → 真实 GuiHostAdapter）。
//!
//! 用 devfixture 把 `fixtures/ui/seed.json`（schema v1）种到隔离 tempdir，
//! 再经与 `examples/ui_fixture.rs` 相同的真实装配（SessionStore 公开写入
//! 路径 + 多 workspace 注册 + checkpoint 服务 + session 绑定）断言 host
//! `snapshot()` / `timeline()` 的投影事实。断言值全部取自 seed.json；
//! 分桶口径与 `apps/desktop/src/projection.rs` 的 `date_bucket` 同源
//!（UTC 日界）。golden 再生步骤见 `fixtures/ui/README.md`。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pawork_app::devfixture::{self, SeedSpec};
use pawork_app::gui_server::GuiHost;
use pawork_app::{AppCore, GuiHostAdapter};
use pawork_domain::{ModelId, ProviderId, SessionId};
use pawork_git::LineKind;
use pawork_protocol::{SnapshotSectionKind, TimelineItem, TimelineItemKind};
use pawork_storage::session::SessionStore;
use pawork_testkit::{MockProvider, MockScript};
use serde_json::Value;

fn fixture_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/ui")
        .join(relative)
}

fn load_spec() -> SeedSpec {
    let text = std::fs::read_to_string(fixture_path("seed.json")).expect("read seed.json");
    serde_json::from_str(&text).expect("parse seed.json")
}

/// 与 desktop `date_bucket` 同一 UTC 日界语义。锚点 `FIXTURE_NOW_MS` 恰为
/// UTC 午夜，因此取锚点前 1ms 作为参照 now：seed 中 -2h/-2.5h 的同日负偏移
/// 落 Today，四桶齐全（desktop 侧期望快照测试用同一口径）。
fn date_bucket(updated_at_ms: u64, now_ms: u64) -> &'static str {
    const DAY_MS: u64 = 86_400_000;
    match now_ms / DAY_MS - updated_at_ms / DAY_MS {
        0 => "today",
        1 => "yesterday",
        2..=7 => "previous_7_days",
        _ => "earlier",
    }
}

fn count_kind(items: &[TimelineItem], kind: TimelineItemKind) -> usize {
    items.iter().filter(|item| item.kind == kind).count()
}

fn is_write_tool(name: &str) -> bool {
    // 与 devfixture::is_write_tool 同集：写工具才产生审批请求/响应事件。
    matches!(name, "write_file" | "edit_file" | "apply_patch")
}

fn section_data<'a>(
    snapshot: &'a pawork_protocol::Snapshot,
    kind: SnapshotSectionKind,
    name: &str,
) -> &'a Vec<Value> {
    snapshot
        .sections
        .iter()
        .find(|section| section.kind == kind)
        .unwrap_or_else(|| panic!("snapshot 缺少 {name} 段"))
        .data
        .as_ref()
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("snapshot {name} 段不是数组"))
}

#[tokio::test]
async fn ui_fixture_seed_to_host_snapshot_and_timeline() {
    let spec = load_spec();
    assert_eq!(spec.workspaces.len(), 3, "seed.json 声明 3 个 workspace");
    assert_eq!(spec.sessions.len(), 7, "seed.json 声明 7 个 session");

    // ---- seed：真实文件系统 + git + SessionStore 公开写入路径 ----
    let root = tempfile::tempdir().expect("tempdir");
    let outcome = devfixture::seed(root.path(), None, &spec, &fixture_path("pty-fixture.sh"))
        .await
        .expect("seed fixture root");
    assert_eq!(outcome.now_ms, spec.now_ms);
    assert_eq!(outcome.now_ms, devfixture::FIXTURE_NOW_MS);
    assert_eq!(outcome.workspaces, 3);
    assert_eq!(outcome.sessions, 7);
    assert!(devfixture::fixture_marker_present(root.path()));
    assert!(root.path().join("manifest.json").is_file());

    let workspaces = devfixture::resolve_workspaces(&spec, root.path()).expect("resolve");
    for entry in &workspaces {
        let declared = spec
            .workspaces
            .iter()
            .find(|item| item.id == entry.id)
            .expect("resolved workspace is declared");
        assert!(entry.path.is_dir(), "{} 未落盘", entry.path.display());
        assert!(
            entry.path.join(".git").exists() == declared.git,
            "{} git 形态与 seed 声明不符",
            entry.path.display()
        );
    }

    // ---- 与 examples/ui_fixture.rs::fixture_core 相同的真实装配 ----
    let (store, _) = SessionStore::open(root.path().join("data/session.db"))
        .await
        .expect("reopen session store");
    let mut core = AppCore::from_parts(
        Arc::new(MockProvider::sequence(vec![MockScript::new()
            .text("unused")
            .complete()])),
        None,
        ModelId::from("fixture-model"),
        ProviderId::from("mock"),
        Some(store),
    );
    devfixture::attach_fixture_workspaces(&mut core, &workspaces).expect("attach workspaces");
    core.open_checkpoints(root.path().join("data/checkpoints"))
        .await
        .expect("open checkpoints");
    devfixture::bind_fixture_sessions(&core, &spec);

    // ---- alpha diff：≥2 文件 + 长行（与 adapter diff 查询同一生产函数）----
    let alpha_id = SessionId::from("fx-ses-alpha-today");
    let alpha = spec
        .sessions
        .iter()
        .find(|session| session.id == "fx-ses-alpha-today")
        .expect("alpha session in seed");
    let diff = core
        .session_diff(&alpha_id)
        .await
        .expect("alpha session diff");
    let expected_paths: BTreeSet<&str> = spec
        .diffs
        .iter()
        .filter(|diff| diff.session_id == "fx-ses-alpha-today")
        .flat_map(|diff| diff.files.iter().map(|file| file.path.as_str()))
        .collect();
    assert!(expected_paths.len() >= 2, "seed 声明 alpha diff ≥2 文件");
    let actual_paths: BTreeSet<&str> = diff.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(
        actual_paths, expected_paths,
        "working tree 与 seed diffs 一致"
    );
    let report = diff
        .files
        .iter()
        .find(|file| file.path == "docs/report.md")
        .expect("docs/report.md in diff");
    assert!(
        report
            .hunks
            .iter()
            .flat_map(|hunk| hunk.lines.iter())
            .any(|line| line.kind == LineKind::Addition && line.text.chars().count() >= 200),
        "docs/report.md 须含 ≥200 字符新增行（横滚样例）"
    );

    // ---- snapshot：workspace 装配 / 7 sessions / pending 重建 ----
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let snapshot = adapter.snapshot().await.expect("host snapshot");

    let workspace_entries = section_data(&snapshot, SnapshotSectionKind::Workspaces, "workspaces");
    assert_eq!(
        workspace_entries.len(),
        1,
        "wire workspaces 段当前只携带主 workspace"
    );
    assert_eq!(
        workspace_entries[0]["id"].as_str(),
        Some(spec.workspaces[0].id.as_str())
    );
    assert_eq!(
        workspace_entries[0]["name"].as_str(),
        Some(spec.workspaces[0].name.as_str())
    );

    let tree = section_data(&snapshot, SnapshotSectionKind::SessionTree, "session_tree");
    let by_id: BTreeMap<&str, &Value> = tree
        .iter()
        .filter_map(|entry| {
            entry
                .get("session_id")
                .and_then(Value::as_str)
                .map(|id| (id, entry))
        })
        .collect();
    assert_eq!(by_id.len(), 7, "snapshot 恰好包含 7 个种子 session");
    for session in &spec.sessions {
        let entry = by_id
            .get(session.id.as_str())
            .unwrap_or_else(|| panic!("snapshot 缺少 session {}", session.id));
        assert_eq!(
            entry["title"].as_str(),
            Some(session.title.as_str()),
            "session {} title",
            session.id
        );
        assert_eq!(
            entry["updated_at_ms"].as_u64(),
            Some((spec.now_ms + session.updated_offset_ms) as u64),
            "session {} updated_at 取事件时间戳",
            session.id
        );
        assert_eq!(
            entry["workspace_id"].as_str(),
            Some(session.workspace_id.as_str()),
            "session {} workspace 绑定",
            session.id
        );
    }

    // 四日期桶分布（值取自 seed.json 的 offset；参照 now = 锚点 - 1ms）。
    let now_ref = (spec.now_ms - 1) as u64;
    let mut buckets: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for session in &spec.sessions {
        let updated = (spec.now_ms + session.updated_offset_ms) as u64;
        buckets
            .entry(date_bucket(updated, now_ref))
            .or_default()
            .push(session.id.as_str());
    }
    assert_eq!(
        buckets.keys().copied().collect::<Vec<_>>(),
        vec!["earlier", "previous_7_days", "today", "yesterday"],
        "种子会话必须铺满四个日期桶"
    );
    assert_eq!(
        buckets.get("today").map(Vec::as_slice),
        Some(["fx-ses-alpha-today", "fx-ses-beta-pending"].as_slice())
    );
    assert_eq!(
        buckets.get("yesterday").map(Vec::as_slice),
        Some(["fx-ses-alpha-yesterday"].as_slice())
    );
    assert_eq!(
        buckets.get("previous_7_days").map(Vec::as_slice),
        Some(["fx-ses-beta-toolfailed", "fx-ses-beta-long"].as_slice())
    );
    assert_eq!(
        buckets.get("earlier").map(Vec::as_slice),
        Some(["fx-ses-beta-cancelled", "fx-ses-alpha-longtitle"].as_slice())
    );

    // pending_approval 重建：仅 beta-pending 的 write_file 停在审批上。
    let pending = section_data(
        &snapshot,
        SnapshotSectionKind::PendingToolApprovals,
        "pending_tool_approvals",
    );
    assert_eq!(
        pending.len(),
        1,
        "只有 seed 声明的 pending_approval 会话待审批"
    );
    assert_eq!(
        pending[0]["session_id"].as_str(),
        Some("fx-ses-beta-pending")
    );
    assert_eq!(
        pending[0]["tool_call_id"].as_str(),
        Some("call-fx-ses-beta-pending-0-0")
    );
    assert_eq!(pending[0]["tool_name"].as_str(), Some("write_file"));
    assert_eq!(pending[0]["relative_path"].as_str(), Some("src/lib.ts"));

    let runs = section_data(&snapshot, SnapshotSectionKind::ActiveRuns, "active_runs");
    assert!(runs.is_empty(), "纯 seed 数据无 live run");

    // ---- timeline：completed 会话条目构成（值全部由 seed turns 推导）----
    let page = adapter
        .timeline(&alpha_id, None, Some(500))
        .await
        .expect("alpha timeline");
    assert!(page.complete, "alpha-today 应单页取全");
    let items = &page.items;
    for pair in items.windows(2) {
        assert!(
            pair[0].sequence < pair[1].sequence,
            "timeline sequence 必须严格递增"
        );
    }
    assert!(matches!(
        items.first(),
        Some(item) if item.kind == TimelineItemKind::UserMessage
    ));
    assert!(matches!(
        items.last(),
        Some(item) if item.kind == TimelineItemKind::RunCompleted
    ));

    let users: Vec<&str> = items
        .iter()
        .filter(|item| item.kind == TimelineItemKind::UserMessage)
        .filter_map(|item| item.text.as_deref())
        .collect();
    assert_eq!(
        users,
        alpha
            .turns
            .iter()
            .map(|turn| turn.user.as_str())
            .collect::<Vec<_>>(),
        "UserMessage 文本逐字等于 seed user 输入（多段 + markdown 列表）"
    );

    let assistants: Vec<String> = items
        .iter()
        .filter(|item| item.kind == TimelineItemKind::AssistantMessage)
        .filter_map(|item| item.text.clone())
        .collect();
    assert_eq!(
        assistants,
        alpha
            .turns
            .iter()
            .map(|turn| turn.assistant.join("\n"))
            .collect::<Vec<_>>(),
        "committed assistant 文本 = 段落以换行拼接（reducer 合并流式 chunk）"
    );
    let expected_deltas: usize = alpha
        .turns
        .iter()
        .map(|turn| {
            if turn.assistant.is_empty() {
                0
            } else {
                turn.stream_chunks.max(1)
            }
        })
        .sum();
    assert_eq!(
        count_kind(items, TimelineItemKind::AssistantDelta),
        expected_deltas,
        "流式 chunk 数由 seed stream_chunks 决定"
    );

    let expected_tools: Vec<&str> = alpha
        .turns
        .iter()
        .flat_map(|turn| turn.tools.iter().map(|tool| tool.name.as_str()))
        .collect();
    let started: Vec<&str> = items
        .iter()
        .filter(|item| item.kind == TimelineItemKind::ToolStarted)
        .filter_map(|item| item.tool_name.as_deref())
        .collect();
    assert_eq!(started, expected_tools, "工具名与顺序逐一对齐 seed turns");
    assert_eq!(
        count_kind(items, TimelineItemKind::ToolOutput),
        expected_tools.len()
    );
    let completed: Vec<(&str, &str)> = items
        .iter()
        .filter(|item| item.kind == TimelineItemKind::ToolCompleted)
        .map(|item| {
            (
                item.tool_name.as_deref().unwrap_or_default(),
                item.status.as_deref().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(completed.len(), expected_tools.len());
    assert!(
        completed.iter().all(|(_, status)| *status == "succeeded"),
        "alpha-today 全部工具 succeeded：{completed:?}"
    );
    let expected_approvals = alpha
        .turns
        .iter()
        .flat_map(|turn| turn.tools.iter())
        .filter(|tool| tool.status == "succeeded" && is_write_tool(&tool.name))
        .count();
    assert_eq!(
        count_kind(items, TimelineItemKind::ApprovalRequested),
        expected_approvals,
        "写工具逐个产生审批请求"
    );
    assert_eq!(
        count_kind(items, TimelineItemKind::ApprovalResponded),
        expected_approvals
    );
    assert_eq!(
        count_kind(items, TimelineItemKind::RunStarted),
        alpha.turns.len()
    );
    assert_eq!(
        count_kind(items, TimelineItemKind::RunCompleted),
        alpha.turns.len()
    );
    assert_eq!(count_kind(items, TimelineItemKind::RunFailed), 0);
    assert_eq!(count_kind(items, TimelineItemKind::RunCancelled), 0);

    // ---- 长会话：≥50 条 timeline 条目（虚拟化压测数据集）----
    let long = spec
        .sessions
        .iter()
        .find(|session| session.id == "fx-ses-beta-long")
        .expect("long session in seed");
    let long_page = adapter
        .timeline(&SessionId::from("fx-ses-beta-long"), None, Some(500))
        .await
        .expect("long timeline");
    assert!(long_page.complete);
    // 每轮无工具：user + run started/completed + (stream_chunks 个 delta + 1 条 committed)。
    let expected_long: usize = long
        .turns
        .iter()
        .map(|turn| {
            3 + if turn.assistant.is_empty() {
                0
            } else {
                turn.stream_chunks.max(1) + 1
            }
        })
        .sum();
    assert_eq!(long_page.items.len(), expected_long);
    assert!(
        long_page.items.len() >= 50,
        "长会话须 ≥50 条 timeline 条目供虚拟化"
    );
}
