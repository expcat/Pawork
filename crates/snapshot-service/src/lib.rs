//! GUI 快照服务（P13-5）：从 [`app_service::AggregateState`] 生成
//! [`gui_protocol::Snapshot`]。
//!
//! 首连握手后与 `SnapshotRequest` 时生成完整 Snapshot；`snapshot_sequence`
//! 取 [`EventHub::current`]，与重连 `Resume` 的 Replay 窗口同一序列空间，
//! 客户端据此判断是否需要补事件。sections：
//!
//! - `Workspaces` / `SessionTree`（按 `forked_from` 建树）/ `ActiveRuns`
//!   （非终态 run）/ `PendingToolApprovals`（Pending 审批）/
//!   `TerminalSessions` / `ProviderStatus`。
//!
//! section 数据全部为有界元数据（内联 `data`，不引用 artifact），大小受
//! [`MAX_SNAPSHOT_SECTION_DATA_BYTES`] 约束（P13-8 之前不拆分大 payload）。

use std::sync::Arc;

use agent_domain::{SessionId, Timestamp};
use app_service::{ApprovalStatus, SessionRecord, Snapshot as AggregateSnapshot};
use core_api::{GlobalSequence, RunState};
use gui_protocol::{
    Snapshot, SnapshotSection, SnapshotSectionKind, MAX_SNAPSHOT_SECTION_DATA_BYTES,
};
use serde_json::{json, Value};
use subscription_hub::EventHub;
use thiserror::Error;

/// 快照构建错误。
#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("snapshot section serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("snapshot section data exceeds {MAX_SNAPSHOT_SECTION_DATA_BYTES} bytes: {0}")]
    SectionTooLarge(String),
}

/// 快照服务：从 app-service 聚合状态 + Event Hub 当前序列生成 Snapshot。
pub struct SnapshotService {
    app_service: Arc<app_service::AppService>,
    hub: Arc<EventHub>,
}

impl SnapshotService {
    pub fn new(app_service: Arc<app_service::AppService>, hub: Arc<EventHub>) -> Self {
        Self { app_service, hub }
    }

    /// 生成完整 Snapshot；`snapshot_sequence` 取 [`EventHub::current`]。
    pub fn build(&self) -> Result<Snapshot, SnapshotError> {
        self.build_with_sequence(self.hub.current())
    }

    /// 以指定 `snapshot_sequence` 生成 Snapshot（测试 / Resume 降级用）。
    pub fn build_with_sequence(
        &self,
        snapshot_sequence: GlobalSequence,
    ) -> Result<Snapshot, SnapshotError> {
        let state = self.app_service.router().aggregate().snapshot();
        let sections = self.build_sections(&state)?;
        Ok(Snapshot {
            instance_id: self.app_service.router().instance_id().clone(),
            snapshot_sequence,
            generated_at: now_timestamp(),
            sections,
        })
    }

    /// 由聚合状态生成全部六个 section（按 [`SnapshotSectionKind`] 顺序）。
    pub fn build_sections(
        &self,
        state: &AggregateSnapshot,
    ) -> Result<Vec<SnapshotSection>, SnapshotError> {
        let revision = self.app_service.router().aggregate().revision();
        let sections = vec![
            section(
                SnapshotSectionKind::Workspaces,
                revision,
                json!({ "workspaces": state.workspaces }),
            ),
            section(
                SnapshotSectionKind::SessionTree,
                revision,
                json!({ "sessions": session_tree(&state.sessions) }),
            ),
            section(
                SnapshotSectionKind::ActiveRuns,
                revision,
                json!({
                    "runs": state
                        .runs
                        .iter()
                        .filter(|run| !is_terminal(&run.state))
                        .collect::<Vec<_>>()
                }),
            ),
            section(
                SnapshotSectionKind::PendingToolApprovals,
                revision,
                json!({
                    "approvals": state
                        .approvals
                        .iter()
                        .filter(|approval| approval.status == ApprovalStatus::Pending)
                        .collect::<Vec<_>>()
                }),
            ),
            section(
                SnapshotSectionKind::TerminalSessions,
                revision,
                json!({ "terminals": state.terminals }),
            ),
            section(
                SnapshotSectionKind::ProviderStatus,
                revision,
                json!({ "providers": state.providers }),
            ),
        ];
        for item in &sections {
            item.validate().map_err(|error| {
                SnapshotError::SectionTooLarge(format!("{:?}: {error}", item.kind))
            })?;
        }
        Ok(sections)
    }
}

fn section(kind: SnapshotSectionKind, revision: u64, data: Value) -> SnapshotSection {
    SnapshotSection {
        kind,
        revision,
        data: Some(data),
        artifact_id: None,
    }
}

/// 会话树：无 `forked_from` 的会话为根，子会话按 `forked_from` 挂接。
fn session_tree(sessions: &[SessionRecord]) -> Vec<Value> {
    let mut children: std::collections::BTreeMap<SessionId, Vec<&SessionRecord>> =
        std::collections::BTreeMap::new();
    let mut roots: Vec<&SessionRecord> = Vec::new();
    for session in sessions {
        match &session.forked_from {
            Some(parent) => children.entry(parent.clone()).or_default().push(session),
            None => roots.push(session),
        }
    }
    fn node(
        session: &SessionRecord,
        children: &std::collections::BTreeMap<SessionId, Vec<&SessionRecord>>,
    ) -> Value {
        json!({
            "session": session,
            "children": children
                .get(&session.session_id)
                .map(|items| items.iter().map(|item| node(item, children)).collect::<Vec<_>>())
                .unwrap_or_default(),
        })
    }
    roots
        .into_iter()
        .map(|root| node(root, &children))
        .collect()
}

fn is_terminal(state: &RunState) -> bool {
    matches!(
        state,
        RunState::Completed | RunState::Cancelled | RunState::Failed | RunState::Interrupted
    )
}

fn now_timestamp() -> Timestamp {
    Timestamp::from_unix_millis(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{EventId, ProviderId, RunId, TerminalSessionId, ToolCallId, WorkspaceId};
    use app_service::AggregateState;
    use core_api::{ApprovalDecision, CommandSource};
    use gui_protocol::{SnapshotSectionKind, MAX_SNAPSHOT_SECTION_DATA_BYTES};
    use std::path::PathBuf;
    use workspace_service::{TrustState, Workspace, WorkspaceRoot};

    fn now(millis: u64) -> Timestamp {
        Timestamp::from_unix_millis(millis)
    }

    fn seed(aggregate: &AggregateState) {
        let workspace_id = WorkspaceId::from("workspace-1");
        aggregate.record_workspace(Workspace {
            id: workspace_id.clone(),
            name: "demo".into(),
            roots: vec![WorkspaceRoot {
                path: PathBuf::from("."),
                git: None,
            }],
            trust: TrustState::Trusted,
            last_accessed_at: now(1),
            revision: 1,
        });
        let session = aggregate
            .create_session(workspace_id.clone(), "root session".into(), now(2))
            .expect("create session");
        aggregate
            .fork_session(&session.session_id, EventId::from("event-1"))
            .expect("fork session");

        let run_id = RunId::from("run-1");
        aggregate
            .record_run(
                run_id.clone(),
                session.session_id.clone(),
                agent_domain::ModelId::from("model-1"),
                ProviderId::from("provider-1"),
                CommandSource::Automation,
                now(3),
            )
            .expect("record run");
        aggregate
            .set_run_state(&run_id, RunState::WaitingForApproval)
            .expect("state");
        let terminal_run = RunId::from("run-2");
        aggregate
            .record_run(
                terminal_run.clone(),
                session.session_id,
                agent_domain::ModelId::from("model-1"),
                ProviderId::from("provider-1"),
                CommandSource::Automation,
                now(4),
            )
            .expect("record terminal run");
        aggregate
            .set_run_state(&terminal_run, RunState::Completed)
            .expect("state");

        aggregate
            .record_approval(
                run_id.clone(),
                ToolCallId::from("tool-1"),
                "pending approval".into(),
                ApprovalStatus::Pending,
            )
            .expect("pending approval");
        aggregate
            .decide_approval(
                &run_id,
                &ToolCallId::from("tool-2"),
                ApprovalDecision::ApproveOnce,
            )
            .expect("decided approval");

        aggregate.record_provider(ProviderId::from("provider-1"), true, 3);
        aggregate.record_provider(ProviderId::from("provider-2"), false, 0);
        aggregate.record_terminal(
            workspace_id,
            TerminalSessionId::from("terminal-1"),
            Some(".".into()),
        );
    }

    #[test]
    fn empty_aggregate_builds_all_sections_and_validates() {
        let hub = EventHub::new();
        assert_eq!(hub.current(), GlobalSequence(0));
        let service = SnapshotService::new(
            Arc::new(app_service::AppService::new("snapshot-test")),
            Arc::new(hub),
        );
        let snapshot = service.build().expect("build");
        assert_eq!(snapshot.snapshot_sequence, GlobalSequence(0));
        assert_eq!(snapshot.instance_id.as_str(), "snapshot-test");
        let kinds: Vec<SnapshotSectionKind> = snapshot
            .sections
            .iter()
            .map(|item| item.kind.clone())
            .collect();
        assert_eq!(
            kinds,
            vec![
                SnapshotSectionKind::Workspaces,
                SnapshotSectionKind::SessionTree,
                SnapshotSectionKind::ActiveRuns,
                SnapshotSectionKind::PendingToolApprovals,
                SnapshotSectionKind::TerminalSessions,
                SnapshotSectionKind::ProviderStatus,
            ]
        );
        snapshot.validate().expect("sections validate");
        for section in &snapshot.sections {
            assert!(section.data.is_some());
            assert!(section.artifact_id.is_none());
        }
    }

    #[test]
    fn sections_reflect_aggregate_state_with_tree_and_filters() {
        let hub = EventHub::new();
        hub.publish(core_api::AppEventEnvelope {
            api_version: core_api::API_VERSION,
            instance_id: agent_domain::CoreInstanceId::from("snapshot-test"),
            event_id: EventId::from("event-1"),
            global_sequence: GlobalSequence(0),
            stream: core_api::EventStream::Global,
            stream_sequence: 1,
            timestamp: now(5),
            source: core_api::EventSource::Core,
            payload: core_api::AppEvent::CoreReady {
                handle: core_api::ApiHandle {
                    instance_id: agent_domain::CoreInstanceId::from("snapshot-test"),
                    api_version: core_api::API_VERSION,
                },
            },
        });
        let app_service = Arc::new(app_service::AppService::new("snapshot-test"));
        seed(app_service.router().aggregate());
        let service = SnapshotService::new(Arc::clone(&app_service), Arc::new(hub));
        let snapshot = service.build().expect("build");
        assert_eq!(snapshot.snapshot_sequence, GlobalSequence(1));
        snapshot.validate().expect("sections validate");

        let by_kind = |kind: &SnapshotSectionKind| {
            snapshot
                .sections
                .iter()
                .find(|section| &section.kind == kind)
                .expect("section")
        };

        // Workspaces：一条。
        let workspaces = by_kind(&SnapshotSectionKind::Workspaces);
        assert_eq!(
            workspaces.data.as_ref().unwrap()["workspaces"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        // SessionTree：根会话挂一个 fork 子会话。
        let tree = by_kind(&SnapshotSectionKind::SessionTree);
        let roots = tree.data.as_ref().unwrap()["sessions"].as_array().unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(
            roots[0]["children"].as_array().unwrap().len(),
            1,
            "fork 子会话必须挂到根会话下"
        );

        // ActiveRuns：仅非终态（run-1），run-2 Completed 被排除。
        let active = by_kind(&SnapshotSectionKind::ActiveRuns);
        let runs = active.data.as_ref().unwrap()["runs"].as_array().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0]["run_id"].as_str().unwrap(), "run-1");

        // PendingToolApprovals：仅 Pending（tool-1），tool-2 已决策被排除。
        let pending = by_kind(&SnapshotSectionKind::PendingToolApprovals);
        let approvals = pending.data.as_ref().unwrap()["approvals"]
            .as_array()
            .unwrap();
        assert_eq!(approvals.len(), 1);
        assert_eq!(approvals[0]["tool_call_id"].as_str().unwrap(), "tool-1");

        // TerminalSessions / ProviderStatus。
        let terminals = by_kind(&SnapshotSectionKind::TerminalSessions);
        assert_eq!(
            terminals.data.as_ref().unwrap()["terminals"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let providers = by_kind(&SnapshotSectionKind::ProviderStatus);
        let provider_list = providers.data.as_ref().unwrap()["providers"]
            .as_array()
            .unwrap();
        assert_eq!(provider_list.len(), 2);
        assert_eq!(provider_list[0]["status"].as_str().unwrap(), "ready");
        assert_eq!(
            provider_list[1]["status"].as_str().unwrap(),
            "authentication_required"
        );

        // build_with_sequence 使用给定序列。
        let snapshot = service
            .build_with_sequence(GlobalSequence(42))
            .expect("build with sequence");
        assert_eq!(snapshot.snapshot_sequence, GlobalSequence(42));
    }

    #[test]
    fn section_data_stays_within_bounds() {
        let app_service = Arc::new(app_service::AppService::new("snapshot-test"));
        seed(app_service.router().aggregate());
        let service = SnapshotService::new(Arc::clone(&app_service), Arc::new(EventHub::new()));
        let snapshot = service.build().expect("build");
        for section in &snapshot.sections {
            let encoded =
                serde_json::to_vec(section.data.as_ref().expect("data")).expect("serialize");
            assert!(
                encoded.len() <= MAX_SNAPSHOT_SECTION_DATA_BYTES,
                "{:?} section exceeded bound",
                section.kind
            );
        }
    }
}
