//! Session 导出 / 导入（P5-8）。
//!
//! 目的：为迁移、备份与 Pi 导入提供稳定的导出 schema，使一个 session 可完整
//! 导出为 JSON 并无损导入回 Event Store（往返等价）。
//!
//! 设计要点：
//! - **稳定 schema**：[`SessionExport`] 自带 `schema_version` 字段，导入时校验；
//!   导出内容包含 session 元信息、分支树、全部事件（事实来源）、标签与投影快照。
//! - **往返等价**：导入后在同一空数据库重建 session，事件 sequence / parent /
//!   branch 结构与导出前完全一致（[`SessionExport::roundtrip_equivalent`] 仅作说明，
//!   等价性由测试断言）。
//! - 导入只追加事件（经 `append_event` 的连续性校验），不破坏 append-only 红线。

use std::collections::{BTreeMap, BTreeSet};

use agent_domain::{SessionId, Timestamp};
use agent_events::AgentEventEnvelope;
use serde::{Deserialize, Serialize};

use crate::{SessionStore, SessionStoreError, DEFAULT_BRANCH_ID};

/// 当前导出 schema 版本。
pub const EXPORT_SCHEMA_VERSION: u32 = 2;

/// 一个分支的导出表示。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportedBranch {
    pub branch_id: String,
    pub parent_branch_id: Option<String>,
    pub forked_from_event_id: Option<String>,
    pub head_sequence: u64,
}

/// 一条事件及其原始 branch 归属。
///
/// `branch_id` 属于 Event Store 的存储维度，并非 [`AgentEventEnvelope`] 的 canonical
/// 字段，因此必须由导出 schema 显式携带，才能在多分支往返时无损恢复。
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExportedEvent {
    pub branch_id: String,
    pub event: AgentEventEnvelope,
}

impl<'de> Deserialize<'de> for ExportedEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum WireEvent {
            V2 {
                branch_id: String,
                event: AgentEventEnvelope,
            },
            /// v1 只携带 envelope，历史导出无法恢复分支归属，安全降级到 main。
            V1(AgentEventEnvelope),
        }

        Ok(match WireEvent::deserialize(deserializer)? {
            WireEvent::V2 { branch_id, event } => Self { branch_id, event },
            WireEvent::V1(event) => Self {
                branch_id: DEFAULT_BRANCH_ID.to_string(),
                event,
            },
        })
    }
}

/// 一个 session 的完整导出（稳定 schema）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionExport {
    /// 导出 schema 版本；当前写 v2，读取兼容 v1～[`EXPORT_SCHEMA_VERSION`]。
    pub schema_version: u32,
    pub session_id: String,
    pub title: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub archived: bool,
    pub active_branch: String,
    /// 分支树（含 main）。
    pub branches: Vec<ExportedBranch>,
    /// 全部事件（事实来源），按 sequence 升序，并携带原始 branch 归属。
    pub events: Vec<ExportedEvent>,
    /// 标签（小写归一）。
    #[serde(default)]
    pub tags: Vec<String>,
}

impl SessionExport {
    /// 序列化为紧凑 JSON 字符串。
    pub fn to_json(&self) -> Result<String, SessionStoreError> {
        serde_json::to_string(self).map_err(SessionStoreError::from)
    }

    /// 从 JSON 反序列化并校验 schema 版本。
    pub fn from_json(json: &str) -> Result<Self, SessionStoreError> {
        let export: SessionExport = serde_json::from_str(json).map_err(SessionStoreError::from)?;
        export.validate()?;
        Ok(export)
    }

    /// 校验 schema 版本。v1 可读并按 main 分支迁移，所有新导出均写 v2。
    pub fn validate(&self) -> Result<(), SessionStoreError> {
        if (1..=EXPORT_SCHEMA_VERSION).contains(&self.schema_version) {
            Ok(())
        } else {
            Err(SessionStoreError::ExportSchemaVersion {
                found: self.schema_version,
                supported: EXPORT_SCHEMA_VERSION,
            })
        }
    }
}

impl SessionStore {
    /// 导出指定 session 为 [`SessionExport`]（事实来源 = 全部事件 + 分支树 + 标签）。
    pub async fn export_session(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionExport, SessionStoreError> {
        let session_id = session_id.to_string();
        self.database()
            .call(move |connection| -> Result<SessionExport, SessionStoreError> {
                let session = connection
                    .query_row(
                        "SELECT session_id, title, created_at_ms, updated_at_ms, archived, active_branch \
                         FROM sessions WHERE session_id=?1",
                        [&session_id],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, i64>(2)?,
                                row.get::<_, i64>(3)?,
                                row.get::<_, i64>(4)?,
                                row.get::<_, String>(5)?,
                            ))
                        },
                    )
                    .map_err(|_| SessionStoreError::SessionNotFound(session_id.clone()))?;
                let (_, title, created, updated, archived, active_branch) = session;

                let branches = {
                    let mut statement = connection.prepare(
                        "SELECT branch_id, parent_branch_id, forked_from_event_id, head_sequence \
                         FROM session_branches WHERE session_id=?1 ORDER BY branch_id",
                    )?;
                    let rows = statement
                        .query_map([&session_id], |row| {
                            Ok(ExportedBranch {
                                branch_id: row.get(0)?,
                                parent_branch_id: row.get(1)?,
                                forked_from_event_id: row.get(2)?,
                                head_sequence: row.get::<_, i64>(3)? as u64,
                            })
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    rows
                };

                let events = {
                    let mut statement = connection.prepare(
                        "SELECT branch_id, payload_json FROM session_events \
                         WHERE session_id=?1 ORDER BY sequence ASC",
                    )?;
                    let rows = statement
                        .query_map([&session_id], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    rows.into_iter()
                        .map(|(branch_id, json)| {
                            let event = serde_json::from_str(&json)
                                .map_err(SessionStoreError::from)?;
                            Ok::<ExportedEvent, SessionStoreError>(ExportedEvent {
                                branch_id,
                                event,
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?
                };

                let tags = {
                    let mut statement = connection
                        .prepare("SELECT tag FROM session_tags WHERE session_id=?1 ORDER BY tag")?;
                    let rows = statement
                        .query_map([&session_id], |row| row.get::<_, String>(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    rows
                };

                Ok(SessionExport {
                    schema_version: EXPORT_SCHEMA_VERSION,
                    session_id,
                    title,
                    created_at_ms: created as u64,
                    updated_at_ms: updated as u64,
                    archived: archived != 0,
                    active_branch,
                    branches,
                    events,
                    tags,
                })
            })
            .await?
    }

    /// 将 [`SessionExport`] 导入当前数据库（在新 / 空 session 上重建）。
    ///
    /// 导入会：创建 session 与分支树、追加全部事件、回填标签。事件经
    /// `append_event` 的连续性 / parent 校验，保持 append-only 红线。若 session 已存在
    /// 则返回错误。
    pub async fn import_session(&self, export: &SessionExport) -> Result<(), SessionStoreError> {
        export.validate()?;

        // 在创建任何数据库状态前先校验 envelope 与导出 session 一致，避免部分导入。
        if let Some(mismatched) = export
            .events
            .iter()
            .find(|exported| exported.event.session_id.as_str() != export.session_id)
        {
            return Err(SessionStoreError::EventSessionMismatch {
                expected_session_id: export.session_id.clone(),
                event_session_id: mismatched.event.session_id.to_string(),
            });
        }

        let session_id = SessionId::from(export.session_id.clone());
        self.create_session(
            &session_id,
            export.title.clone(),
            Timestamp::from_unix_millis(export.created_at_ms),
        )
        .await?;

        let branches: BTreeMap<&str, &ExportedBranch> = export
            .branches
            .iter()
            .map(|branch| (branch.branch_id.as_str(), branch))
            .collect();
        let mut created_branches = BTreeSet::from([DEFAULT_BRANCH_ID.to_string()]);

        // 按全局 sequence 追加事件。分支在首次事件前延迟创建，确保其 fork event 已落库；
        // 每条事件写入前切换 active branch，以兑现 append_event 的并发写保护。
        for exported in &export.events {
            if !created_branches.contains(&exported.branch_id) {
                let branch = branches.get(exported.branch_id.as_str()).ok_or_else(|| {
                    SessionStoreError::BranchNotFound {
                        session_id: export.session_id.clone(),
                        branch_id: exported.branch_id.clone(),
                    }
                })?;
                self.create_branch(
                    &session_id,
                    branch.branch_id.clone(),
                    branch.parent_branch_id.clone(),
                    branch.forked_from_event_id.clone(),
                )
                .await?;
                created_branches.insert(branch.branch_id.clone());
            }
            self.switch_branch(&session_id, exported.branch_id.clone())
                .await?;
            self.append_event(exported.branch_id.clone(), exported.event.clone())
                .await?;
        }

        // 无事件分支也必须恢复；此时全部可能的 fork event 已落库。
        let mut remaining: Vec<&ExportedBranch> = export
            .branches
            .iter()
            .filter(|branch| !created_branches.contains(&branch.branch_id))
            .collect();
        remaining.sort_by_key(|branch| branch.head_sequence);
        for branch in remaining {
            self.create_branch(
                &session_id,
                branch.branch_id.clone(),
                branch.parent_branch_id.clone(),
                branch.forked_from_event_id.clone(),
            )
            .await?;
            created_branches.insert(branch.branch_id.clone());
        }

        // 导入过程会多次切换，最终恢复导出时的 active branch。
        self.switch_branch(&session_id, export.active_branch.clone())
            .await?;

        // 回填标签。
        if !export.tags.is_empty() {
            let tag_refs: Vec<&str> = export.tags.iter().map(String::as_str).collect();
            self.add_tags(&session_id, &tag_refs).await?;
        }

        // 回填 archived 状态（create_session 默认未归档）。
        if export.archived {
            self.archive_session(&session_id).await?;
        }

        Ok(())
    }
}

/// 导入报告：记录导入的事件数、跳过项与未知字段（Pi 导入器扩展使用）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub imported_events: usize,
    pub imported_branches: usize,
    pub imported_tags: usize,
    /// 未识别字段（key -> 原始 JSON 值的字符串表示）。
    pub unknown_fields: BTreeMap<String, String>,
}

impl ImportReport {
    pub fn record_unknown(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.unknown_fields.insert(key.into(), value.into());
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use agent_domain::{
        ContentPart, EventId, Message, MessageId, MessageMetadata, MessageRole, RunId, TextContent,
        Timestamp, ToolCallId,
    };
    use agent_events::{AgentEvent, AgentEventEnvelope, EventSequence};

    use super::*;
    use crate::SessionStore;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_path() -> PathBuf {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pawork-export-{}-{unique}.sqlite3",
            std::process::id()
        ))
    }

    fn event(session: &SessionId, seq: u64, payload: AgentEvent) -> AgentEventEnvelope {
        AgentEventEnvelope::new(
            EventId::from(format!("event-{seq}")),
            session.clone(),
            RunId::from("run-1"),
            EventSequence::new(seq),
            Timestamp::from_unix_millis(1000 + seq),
            payload,
        )
    }

    async fn build_session(store: &SessionStore) -> SessionId {
        let session = SessionId::from("session-export");
        store
            .create_session(&session, "export demo", Timestamp::from_unix_millis(1))
            .await
            .expect("session");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    1,
                    AgentEvent::MessageCommitted {
                        message: Message {
                            id: MessageId::from("m1"),
                            role: MessageRole::User,
                            content: vec![ContentPart::Text(TextContent { text: "hi".into() })],
                            metadata: MessageMetadata::default(),
                        },
                    },
                ),
            )
            .await
            .expect("append 1");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    2,
                    AgentEvent::ToolCallStarted {
                        tool_call_id: ToolCallId::from("tool-1"),
                        name: "read_file".into(),
                    },
                ),
            )
            .await
            .expect("append 2");
        store
            .add_tags(&session, &["rust", "demo"])
            .await
            .expect("tags");
        session
    }

    #[tokio::test]
    async fn export_round_trips_through_json_and_import() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = build_session(&store).await;

        let export = store.export_session(&session).await.expect("export");
        assert_eq!(export.schema_version, EXPORT_SCHEMA_VERSION);
        assert_eq!(export.session_id, "session-export");
        assert_eq!(export.events.len(), 2);
        assert_eq!(export.tags, vec!["demo", "rust"]);
        assert_eq!(export.active_branch, DEFAULT_BRANCH_ID);

        let json = export.to_json().expect("to_json");
        let decoded = SessionExport::from_json(&json).expect("from_json");
        assert_eq!(decoded, export);

        // 导入到新数据库。
        let path2 = temp_path();
        let (store2, _) = SessionStore::open(&path2).await.expect("store2");
        store2.import_session(&export).await.expect("import");

        let re_exported = store2.export_session(&session).await.expect("re-export");
        // 事件与分支结构等价（created/updated ms 可能因导入顺序不同，故只比对核心事实）。
        assert_eq!(re_exported.events, export.events);
        assert_eq!(re_exported.branches, export.branches);
        assert_eq!(re_exported.tags, export.tags);
        assert_eq!(re_exported.title, export.title);

        store.shutdown().await.expect("shutdown");
        store2.shutdown().await.expect("shutdown2");
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(path2);
    }

    #[tokio::test]
    async fn multi_branch_export_import_preserves_event_branch_ownership() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-multi-branch-export");
        store
            .create_session(&session, "branches", Timestamp::from_unix_millis(1))
            .await
            .expect("session");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    1,
                    AgentEvent::CompactionStarted {
                        source_event_count: 1,
                    },
                ),
            )
            .await
            .expect("main event 1");
        store
            .create_branch(
                &session,
                "experiment",
                Some(DEFAULT_BRANCH_ID.into()),
                Some("event-1".into()),
            )
            .await
            .expect("branch");
        store
            .switch_branch(&session, "experiment")
            .await
            .expect("switch experiment");
        store
            .append_event(
                "experiment",
                event(
                    &session,
                    2,
                    AgentEvent::CompactionStarted {
                        source_event_count: 2,
                    },
                ),
            )
            .await
            .expect("experiment event");
        store
            .switch_branch(&session, DEFAULT_BRANCH_ID)
            .await
            .expect("switch main");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    3,
                    AgentEvent::CompactionStarted {
                        source_event_count: 3,
                    },
                ),
            )
            .await
            .expect("main event 3");

        let export = store.export_session(&session).await.expect("export");
        assert_eq!(
            export
                .events
                .iter()
                .map(|event| (event.event.sequence.value(), event.branch_id.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (1, DEFAULT_BRANCH_ID),
                (2, "experiment"),
                (3, DEFAULT_BRANCH_ID)
            ]
        );

        let path2 = temp_path();
        let (store2, _) = SessionStore::open(&path2).await.expect("store2");
        store2.import_session(&export).await.expect("import");
        let re_exported = store2.export_session(&session).await.expect("re-export");
        assert_eq!(re_exported.events, export.events);
        assert_eq!(re_exported.branches, export.branches);
        assert_eq!(re_exported.active_branch, export.active_branch);
        assert_eq!(
            store2
                .events_by_branch(&session, "experiment", 1, 10)
                .await
                .expect("experiment events")
                .iter()
                .map(|event| event.sequence.value())
                .collect::<Vec<_>>(),
            vec![2]
        );

        store.shutdown().await.expect("shutdown");
        store2.shutdown().await.expect("shutdown2");
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(path2);
    }

    #[tokio::test]
    async fn import_rejects_unsupported_schema_version() {
        let mut export = SessionExport {
            schema_version: 999,
            session_id: "x".into(),
            title: "t".into(),
            created_at_ms: 1,
            updated_at_ms: 1,
            archived: false,
            active_branch: DEFAULT_BRANCH_ID.into(),
            branches: vec![ExportedBranch {
                branch_id: DEFAULT_BRANCH_ID.into(),
                parent_branch_id: None,
                forked_from_event_id: None,
                head_sequence: 0,
            }],
            events: vec![],
            tags: vec![],
        };
        let json = serde_json::to_string(&export).expect("serialize");
        assert!(SessionExport::from_json(&json).is_err());
        export.schema_version = EXPORT_SCHEMA_VERSION;
        assert!(export.validate().is_ok());
    }

    #[test]
    fn schema_v1_json_migrates_events_to_main_branch() {
        let session = SessionId::from("legacy-session");
        let legacy = serde_json::json!({
            "schema_version": 1,
            "session_id": session.as_str(),
            "title": "legacy",
            "created_at_ms": 1,
            "updated_at_ms": 1,
            "archived": false,
            "active_branch": DEFAULT_BRANCH_ID,
            "branches": [{
                "branch_id": DEFAULT_BRANCH_ID,
                "parent_branch_id": null,
                "forked_from_event_id": null,
                "head_sequence": 1
            }],
            "events": [event(&session, 1, AgentEvent::RunCancelled { reason: None })],
            "tags": []
        });
        let decoded = SessionExport::from_json(&legacy.to_string()).expect("read v1");
        assert_eq!(decoded.schema_version, 1);
        assert_eq!(decoded.events[0].branch_id, DEFAULT_BRANCH_ID);
    }

    #[tokio::test]
    async fn import_rejects_event_from_another_session_before_creating_state() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let expected = SessionId::from("expected-session");
        let other = SessionId::from("other-session");
        let export = SessionExport {
            schema_version: EXPORT_SCHEMA_VERSION,
            session_id: expected.to_string(),
            title: "mismatch".into(),
            created_at_ms: 1,
            updated_at_ms: 1,
            archived: false,
            active_branch: DEFAULT_BRANCH_ID.into(),
            branches: vec![ExportedBranch {
                branch_id: DEFAULT_BRANCH_ID.into(),
                parent_branch_id: None,
                forked_from_event_id: None,
                head_sequence: 1,
            }],
            events: vec![ExportedEvent {
                branch_id: DEFAULT_BRANCH_ID.into(),
                event: event(&other, 1, AgentEvent::RunCancelled { reason: None }),
            }],
            tags: vec![],
        };

        let error = store.import_session(&export).await.expect_err("mismatch");
        assert!(matches!(
            error,
            SessionStoreError::EventSessionMismatch {
                expected_session_id,
                event_session_id,
            } if expected_session_id == "expected-session" && event_session_id == "other-session"
        ));
        assert!(matches!(
            store.export_session(&expected).await,
            Err(SessionStoreError::SessionNotFound(_))
        ));

        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn export_errors_when_session_missing() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let missing = SessionId::from("does-not-exist");
        let err = store.export_session(&missing).await;
        assert!(matches!(err, Err(SessionStoreError::SessionNotFound(_))));
        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }
}
