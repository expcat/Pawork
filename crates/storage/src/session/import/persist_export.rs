//! Pawork 导出 / 导入的 store 写入。
//!
//! 导出 JSON 形状冻结在 [`crate::session::import::formats::export`]；本模块负责读取事实表
//! 与同一 Immediate 事务整批写入（失败回滚）。Pawork export/import 往返不扫 Secret 前缀。

use std::collections::{BTreeMap, BTreeSet};

use pawork_domain::{PrincipalId, SessionId, TenantId};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};

use crate::session::event_store::persist_event_in_transaction;
use crate::session::import::formats::export::{
    ExportedBranch, ExportedEvent, SessionExport, EXPORT_SCHEMA_VERSION,
};
use crate::session::{SessionStore, SessionStoreError, DEFAULT_BRANCH_ID};

impl SessionStore {
    /// 为 session 添加标签（去重、小写归一）。重复标签静默忽略。
    pub async fn add_tags(
        &self,
        session_id: &SessionId,
        tags: &[&str],
    ) -> Result<(), SessionStoreError> {
        let session_id = session_id.to_string();
        let normalized: Vec<String> = tags
            .iter()
            .map(|tag| tag.trim().to_ascii_lowercase())
            .filter(|tag| !tag.is_empty())
            .collect();
        self.database()
            .call(move |connection| -> Result<(), SessionStoreError> {
                for tag in normalized {
                    connection.execute(
                        "INSERT OR IGNORE INTO session_tags(session_id, tag) VALUES (?1, ?2)",
                        params![session_id, tag],
                    )?;
                }
                Ok(())
            })
            .await??;
        Ok(())
    }

    /// 读取 session 的身份归属；不存在返回 `None`。
    pub async fn get_session_identity(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<(TenantId, PrincipalId)>, SessionStoreError> {
        let session_id = session_id.to_string();
        self.database()
            .call(
                move |connection| -> Result<Option<(TenantId, PrincipalId)>, SessionStoreError> {
                    let row = connection
                        .query_row(
                            "SELECT tenant_id, principal_id FROM sessions WHERE session_id=?1",
                            [&session_id],
                            |row| {
                                Ok((
                                    TenantId::new(row.get::<_, String>(0)?),
                                    PrincipalId::new(row.get::<_, String>(1)?),
                                ))
                            },
                        )
                        .optional()?;
                    Ok(row)
                },
            )
            .await?
    }

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
                        "SELECT session_id, title, created_at_ms, updated_at_ms, archived, active_branch, tenant_id, principal_id \
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
                                row.get::<_, String>(6)?,
                                row.get::<_, String>(7)?,
                            ))
                        },
                    )
                    .map_err(|_| SessionStoreError::SessionNotFound(session_id.clone()))?;
                let (_, title, created, updated, archived, active_branch, tenant_id, principal_id) =
                    session;

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
                    tenant_id: TenantId::new(tenant_id),
                    principal_id: PrincipalId::new(principal_id),
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
    /// 同一 Immediate 事务创建 session / 分支 / 事件 / 标签 / 归档状态；任一失败整批回滚。
    /// 若 session 已存在则返回错误。不扫描 Secret 前缀（事件已在 store 边界脱敏）。
    pub async fn import_session(
        &self,
        export: &SessionExport,
        tenant_id: &TenantId,
        principal_id: &PrincipalId,
    ) -> Result<(), SessionStoreError> {
        export.validate()?;
        if export.tenant_id != *tenant_id || export.principal_id != *principal_id {
            return Err(SessionStoreError::ExportIdentityMismatch {
                export_tenant: export.tenant_id.to_string(),
                export_principal: export.principal_id.to_string(),
                import_tenant: tenant_id.to_string(),
                import_principal: principal_id.to_string(),
            });
        }

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

        let export = export.clone();
        let tenant_id = tenant_id.to_string();
        let principal_id = principal_id.to_string();
        self.database()
            .call(move |connection| -> Result<(), SessionStoreError> {
                let transaction =
                    connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                persist_export_in_transaction(&transaction, &export, &tenant_id, &principal_id)?;
                transaction.commit()?;
                Ok(())
            })
            .await??;
        Ok(())
    }
}

fn persist_export_in_transaction(
    transaction: &Transaction<'_>,
    export: &SessionExport,
    tenant_id: &str,
    principal_id: &str,
) -> Result<(), SessionStoreError> {
    let created = i64::try_from(export.created_at_ms).map_err(|_| {
        SessionStoreError::ProjectionInvariant("timestamp exceeds SQLite INTEGER".into())
    })?;
    transaction.execute(
        "INSERT INTO sessions(session_id, title, created_at_ms, updated_at_ms, tenant_id, principal_id) \
         VALUES (?1, ?2, ?3, ?3, ?4, ?5)",
        params![
            export.session_id,
            export.title,
            created,
            tenant_id,
            principal_id
        ],
    )?;
    transaction.execute(
        "INSERT INTO session_branches(branch_id, session_id, head_sequence) VALUES (?1, ?2, 0)",
        params![DEFAULT_BRANCH_ID, export.session_id],
    )?;

    let branches: BTreeMap<&str, &ExportedBranch> = export
        .branches
        .iter()
        .map(|branch| (branch.branch_id.as_str(), branch))
        .collect();
    let mut created_branches = BTreeSet::from([DEFAULT_BRANCH_ID.to_string()]);

    for exported in &export.events {
        if !created_branches.contains(&exported.branch_id) {
            let branch = branches.get(exported.branch_id.as_str()).ok_or_else(|| {
                SessionStoreError::BranchNotFound {
                    session_id: export.session_id.clone(),
                    branch_id: exported.branch_id.clone(),
                }
            })?;
            insert_branch_in_transaction(transaction, &export.session_id, branch)?;
            created_branches.insert(branch.branch_id.clone());
        }
        persist_imported_event(transaction, &exported.branch_id, &exported.event)?;
    }

    let mut remaining: Vec<&ExportedBranch> = export
        .branches
        .iter()
        .filter(|branch| !created_branches.contains(&branch.branch_id))
        .collect();
    remaining.sort_by_key(|branch| branch.head_sequence);
    for branch in remaining {
        insert_branch_in_transaction(transaction, &export.session_id, branch)?;
        created_branches.insert(branch.branch_id.clone());
    }

    let branch_exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM session_branches WHERE session_id=?1 AND branch_id=?2)",
        params![export.session_id, export.active_branch],
        |row| row.get(0),
    )?;
    if !branch_exists {
        return Err(SessionStoreError::BranchNotFound {
            session_id: export.session_id.clone(),
            branch_id: export.active_branch.clone(),
        });
    }
    transaction.execute(
        "UPDATE sessions SET active_branch=?1, archived=?2 WHERE session_id=?3",
        params![
            export.active_branch,
            if export.archived { 1i64 } else { 0 },
            export.session_id
        ],
    )?;

    for tag in &export.tags {
        let normalized = tag.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            continue;
        }
        transaction.execute(
            "INSERT OR IGNORE INTO session_tags(session_id, tag) VALUES (?1, ?2)",
            params![export.session_id, normalized],
        )?;
    }
    Ok(())
}

fn insert_branch_in_transaction(
    transaction: &Transaction<'_>,
    session_id: &str,
    branch: &ExportedBranch,
) -> Result<(), SessionStoreError> {
    let existing: Option<(Option<String>, Option<String>)> = transaction
        .query_row(
            "SELECT parent_branch_id, forked_from_event_id FROM session_branches \
             WHERE session_id=?1 AND branch_id=?2",
            params![session_id, branch.branch_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((existing_parent, existing_fork)) = existing {
        if existing_parent == branch.parent_branch_id
            && existing_fork == branch.forked_from_event_id
        {
            return Ok(());
        }
        return Err(SessionStoreError::BranchAlreadyExists {
            session_id: session_id.to_string(),
            branch_id: branch.branch_id.clone(),
        });
    }
    let head_sequence = if let Some(event_id) = branch.forked_from_event_id.as_deref() {
        transaction
            .query_row(
                "SELECT sequence FROM session_events WHERE session_id=?1 AND event_id=?2",
                params![session_id, event_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .ok_or_else(|| SessionStoreError::ParentEventNotFound(event_id.into()))?
    } else {
        0
    };
    transaction.execute(
        "INSERT INTO session_branches(branch_id, session_id, parent_branch_id, forked_from_event_id, head_sequence) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            branch.branch_id,
            session_id,
            branch.parent_branch_id,
            branch.forked_from_event_id,
            head_sequence
        ],
    )?;
    Ok(())
}

fn persist_imported_event(
    transaction: &Transaction<'_>,
    branch_id: &str,
    event: &pawork_domain::AgentEventEnvelope,
) -> Result<(), SessionStoreError> {
    let session_id = event.session_id.to_string();
    let sequence = event.sequence.value();
    let previous: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(sequence), 0) FROM session_events WHERE session_id=?1",
        [&session_id],
        |row| row.get(0),
    )?;
    let expected = u64::try_from(previous)
        .ok()
        .and_then(|value| value.checked_add(1))
        .ok_or(SessionStoreError::SequenceOverflow)?;
    if sequence != expected {
        return Err(SessionStoreError::NonContiguousSequence {
            expected,
            actual: sequence,
        });
    }
    if let Some(parent) = event.parent_event_id.as_ref() {
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_events WHERE session_id=?1 AND event_id=?2)",
            params![session_id, parent.to_string()],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(SessionStoreError::ParentEventNotFound(parent.to_string()));
        }
    }
    persist_event_in_transaction(transaction, branch_id, event)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use pawork_domain::{
        AgentEvent, AgentEventEnvelope, ContentPart, EventId, EventSequence, Message, MessageId,
        MessageMetadata, MessageRole, PrincipalId, RunId, SessionId, TenantId, TextContent,
        Timestamp, ToolCallId,
    };

    use super::*;
    use crate::session::import::formats::export::{LEGACY_PRINCIPAL, LEGACY_TENANT};
    use crate::session::SessionStore;

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
        assert_eq!(export.tenant_id.as_str(), LEGACY_TENANT);
        assert_eq!(export.principal_id.as_str(), LEGACY_PRINCIPAL);
        assert_eq!(export.tags, vec!["demo", "rust"]);
        assert_eq!(export.active_branch, DEFAULT_BRANCH_ID);

        let json = export.to_json().expect("to_json");
        let decoded = SessionExport::from_json(&json).expect("from_json");
        assert_eq!(decoded, export);

        // 导入到新数据库。
        let path2 = temp_path();
        let (store2, _) = SessionStore::open(&path2).await.expect("store2");
        store2
            .import_session(&export, &export.tenant_id, &export.principal_id)
            .await
            .expect("import");

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
        store2
            .import_session(&export, &export.tenant_id, &export.principal_id)
            .await
            .expect("import");
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
    async fn import_rejects_event_from_another_session_before_creating_state() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let expected = SessionId::from("expected-session");
        let other = SessionId::from("other-session");
        let export = SessionExport {
            schema_version: EXPORT_SCHEMA_VERSION,
            session_id: expected.to_string(),
            tenant_id: TenantId::new(LEGACY_TENANT),
            principal_id: PrincipalId::new(LEGACY_PRINCIPAL),
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
                event: event(
                    &other,
                    1,
                    AgentEvent::RunCancelled {
                        reason: None,
                        usage: None,
                    },
                ),
            }],
            tags: vec![],
        };

        let error = store
            .import_session(&export, &export.tenant_id, &export.principal_id)
            .await
            .expect_err("mismatch");
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

    #[tokio::test]
    async fn import_rejects_identity_mismatch_before_creating_session() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let export = SessionExport {
            schema_version: EXPORT_SCHEMA_VERSION,
            session_id: "tenant-a-session".into(),
            tenant_id: TenantId::new("tenant-a"),
            principal_id: PrincipalId::new("principal-a"),
            title: "tenant a".into(),
            created_at_ms: 1,
            updated_at_ms: 1,
            archived: false,
            active_branch: DEFAULT_BRANCH_ID.into(),
            branches: vec![],
            events: vec![],
            tags: vec![],
        };
        let error = store
            .import_session(
                &export,
                &TenantId::new("tenant-b"),
                &PrincipalId::new("principal-b"),
            )
            .await
            .expect_err("cross-identity import must fail");
        assert!(matches!(
            error,
            SessionStoreError::ExportIdentityMismatch { .. }
        ));
        assert_eq!(
            store
                .get_session_identity(&SessionId::from("tenant-a-session"))
                .await
                .expect("identity lookup"),
            None
        );
        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn import_rolls_back_entire_batch_on_persist_failure() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("export-rollback");
        let export = SessionExport {
            schema_version: EXPORT_SCHEMA_VERSION,
            session_id: session.to_string(),
            tenant_id: TenantId::new(LEGACY_TENANT),
            principal_id: PrincipalId::new(LEGACY_PRINCIPAL),
            title: "rollback".into(),
            created_at_ms: 1,
            updated_at_ms: 1,
            archived: false,
            active_branch: DEFAULT_BRANCH_ID.into(),
            branches: vec![ExportedBranch {
                branch_id: DEFAULT_BRANCH_ID.into(),
                parent_branch_id: None,
                forked_from_event_id: None,
                head_sequence: 2,
            }],
            events: vec![
                ExportedEvent {
                    branch_id: DEFAULT_BRANCH_ID.into(),
                    event: event(
                        &session,
                        1,
                        AgentEvent::CompactionStarted {
                            source_event_count: 1,
                        },
                    ),
                },
                ExportedEvent {
                    branch_id: DEFAULT_BRANCH_ID.into(),
                    event: AgentEventEnvelope::new(
                        EventId::from("event-1"),
                        session.clone(),
                        RunId::from("run-1"),
                        EventSequence::new(2),
                        Timestamp::from_unix_millis(1002),
                        AgentEvent::CompactionStarted {
                            source_event_count: 2,
                        },
                    ),
                },
            ],
            tags: vec![],
        };

        let error = store
            .import_session(&export, &export.tenant_id, &export.principal_id)
            .await
            .expect_err("duplicate event id must abort");
        assert!(matches!(error, SessionStoreError::Sqlite(..)));
        assert!(matches!(
            store.export_session(&session).await,
            Err(SessionStoreError::SessionNotFound(_))
        ));

        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }
}
