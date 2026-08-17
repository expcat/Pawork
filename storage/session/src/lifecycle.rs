//! Session 生命周期：重命名 / 归档 / 恢复 / 删除；并发占用租约；只读完整性检测。
//!
//! [`SessionStore::get_session_identity`] 已在 export 写入模块实现，此处不复制。

use std::time::{SystemTime, UNIX_EPOCH};

use pawork_domain::SessionId;
use rusqlite::{params, OptionalExtension};

use crate::{SessionStore, SessionStoreError};

/// 租约获取或续期成功后的凭据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseReceipt {
    pub holder: String,
    pub acquired_at_ms: i64,
    pub expires_at_ms: i64,
}

/// sequence 流中的缺失区间（左闭右闭）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SequenceGap {
    /// 缺失区间的第一个 sequence。
    pub from: u64,
    /// 缺失区间的最后一个 sequence。
    pub to: u64,
}

/// parent_event_id 指向不存在事件的损坏记录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingParent {
    pub event_id: String,
    pub parent_event_id: String,
}

/// 只读完整性检测结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrityReport {
    pub session_id: String,
    pub ok: bool,
    pub event_count: u64,
    pub max_sequence: u64,
    pub sequence_gaps: Vec<SequenceGap>,
    pub missing_parents: Vec<MissingParent>,
}

fn now_unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

impl SessionStore {
    /// 重命名 session。
    pub async fn rename_session(
        &self,
        session_id: &SessionId,
        new_title: impl Into<String>,
    ) -> Result<(), SessionStoreError> {
        let session_id = session_id.to_string();
        let new_title = new_title.into();
        let now = now_unix_millis();
        self.database()
            .call(move |connection| -> Result<(), SessionStoreError> {
                let updated = connection.execute(
                    "UPDATE sessions SET title=?1, updated_at_ms=?2 WHERE session_id=?3",
                    params![new_title, now, session_id],
                )?;
                if updated == 0 {
                    return Err(SessionStoreError::SessionNotFound(session_id));
                }
                Ok(())
            })
            .await?
    }

    /// 归档 session（隐藏、冻结，不删除数据）。
    pub async fn archive_session(&self, session_id: &SessionId) -> Result<(), SessionStoreError> {
        self.set_archived(session_id, 1).await
    }

    /// 取消归档 session。
    pub async fn unarchive_session(&self, session_id: &SessionId) -> Result<(), SessionStoreError> {
        self.set_archived(session_id, 0).await
    }

    async fn set_archived(
        &self,
        session_id: &SessionId,
        archived: i64,
    ) -> Result<(), SessionStoreError> {
        let session_id = session_id.to_string();
        self.database()
            .call(move |connection| -> Result<(), SessionStoreError> {
                let updated = connection.execute(
                    "UPDATE sessions SET archived=?1 WHERE session_id=?2",
                    params![archived, session_id],
                )?;
                if updated == 0 {
                    return Err(SessionStoreError::SessionNotFound(session_id));
                }
                Ok(())
            })
            .await?
    }

    /// 恢复会话以承接新 run：释放任何占用租约（若存在）。
    pub async fn resume_session(&self, session_id: &SessionId) -> Result<(), SessionStoreError> {
        let session_id = session_id.to_string();
        self.database()
            .call(move |connection| -> Result<(), SessionStoreError> {
                let transaction = connection.transaction()?;
                let exists: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id=?1)",
                    [&session_id],
                    |row| row.get(0),
                )?;
                if !exists {
                    return Err(SessionStoreError::SessionNotFound(session_id));
                }
                transaction.execute(
                    "DELETE FROM session_leases WHERE session_id=?1",
                    [&session_id],
                )?;
                transaction.commit()?;
                Ok(())
            })
            .await?
    }

    /// 删除 session。
    ///
    /// 由于 `session_events` 是 append-only 且对 sessions 有 `ON DELETE RESTRICT` 外键，
    /// 只有尚无事件的 session 可被硬删除（用于清理空会话）。已有事件的 session 应归档，
    /// 硬删除会返回 [`SessionStoreError::SessionHasEvents`]。
    pub async fn delete_session(&self, session_id: &SessionId) -> Result<(), SessionStoreError> {
        let session_id = session_id.to_string();
        self.database()
            .call(move |connection| -> Result<(), SessionStoreError> {
                let transaction = connection.transaction()?;
                let exists: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id=?1)",
                    [&session_id],
                    |row| row.get(0),
                )?;
                if !exists {
                    return Err(SessionStoreError::SessionNotFound(session_id));
                }
                let has_events: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM session_events WHERE session_id=?1)",
                    [&session_id],
                    |row| row.get(0),
                )?;
                if has_events {
                    return Err(SessionStoreError::SessionHasEvents { session_id });
                }
                // 无事件：FK 级联清理 branches/messages/runs/tool_calls/leases；
                // events 不存在，故不触发 RESTRICT。
                transaction.execute("DELETE FROM sessions WHERE session_id=?1", [&session_id])?;
                transaction.commit()?;
                Ok(())
            })
            .await?
    }

    /// 获取 session 的并发占用租约。
    ///
    /// 若已有未过期租约且持有者非本次请求，返回 [`SessionStoreError::LeaseHeld`]；
    /// 已过期的租约可被抢占。
    pub async fn acquire_lease(
        &self,
        session_id: &SessionId,
        holder: impl Into<String>,
        duration_ms: u64,
    ) -> Result<LeaseReceipt, SessionStoreError> {
        let session_id = session_id.to_string();
        let holder = holder.into();
        let now = now_unix_millis();
        let duration = i64::try_from(duration_ms).unwrap_or(i64::MAX);
        let expires_at = now.checked_add(duration).unwrap_or(i64::MAX);
        self.database()
            .call(
                move |connection| -> Result<LeaseReceipt, SessionStoreError> {
                    let transaction = connection.transaction()?;
                    let exists: bool = transaction.query_row(
                        "SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id=?1)",
                        [&session_id],
                        |row| row.get(0),
                    )?;
                    if !exists {
                        return Err(SessionStoreError::SessionNotFound(session_id));
                    }
                    let current: Option<(String, i64)> = transaction
                        .query_row(
                            "SELECT holder, expires_at_ms FROM session_leases WHERE session_id=?1",
                            [&session_id],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                        .optional()?;
                    if let Some((current_holder, current_expires)) = current {
                        if current_expires > now {
                            return Err(SessionStoreError::LeaseHeld {
                                session_id,
                                holder: current_holder,
                                expires_at_ms: current_expires,
                            });
                        }
                    }
                    transaction.execute(
                    "INSERT INTO session_leases(session_id, holder, acquired_at_ms, expires_at_ms) \
                     VALUES (?1, ?2, ?3, ?4) \
                     ON CONFLICT(session_id) DO UPDATE SET \
                     holder=excluded.holder, \
                     acquired_at_ms=excluded.acquired_at_ms, \
                     expires_at_ms=excluded.expires_at_ms",
                    params![session_id, holder, now, expires_at],
                )?;
                    transaction.commit()?;
                    Ok(LeaseReceipt {
                        holder,
                        acquired_at_ms: now,
                        expires_at_ms: expires_at,
                    })
                },
            )
            .await?
    }

    /// 续期当前持有者的租约；过期租约也可由其持有者续期。
    ///
    /// 仅当前持有者可续期；若被他人未过期占用，返回 [`SessionStoreError::LeaseHeld`]；
    /// 若无租约，返回 [`SessionStoreError::LeaseNotHeld`]。
    pub async fn renew_lease(
        &self,
        session_id: &SessionId,
        holder: impl Into<String>,
        duration_ms: u64,
    ) -> Result<LeaseReceipt, SessionStoreError> {
        let session_id = session_id.to_string();
        let holder = holder.into();
        let now = now_unix_millis();
        let duration = i64::try_from(duration_ms).unwrap_or(i64::MAX);
        let expires_at = now.checked_add(duration).unwrap_or(i64::MAX);
        self.database()
            .call(
                move |connection| -> Result<LeaseReceipt, SessionStoreError> {
                    let transaction = connection.transaction()?;
                    let current: Option<(String, i64, i64)> = transaction
                        .query_row(
                            "SELECT holder, acquired_at_ms, expires_at_ms \
                         FROM session_leases WHERE session_id=?1",
                            [&session_id],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                        )
                        .optional()?;
                    let (current_holder, acquired_at_ms, current_expires) = match current {
                        Some(value) => value,
                        None => return Err(SessionStoreError::LeaseNotHeld { session_id }),
                    };
                    if current_holder != holder {
                        return Err(SessionStoreError::LeaseHeld {
                            session_id,
                            holder: current_holder,
                            expires_at_ms: current_expires,
                        });
                    }
                    transaction.execute(
                        "UPDATE session_leases SET expires_at_ms=?1 WHERE session_id=?2",
                        params![expires_at, session_id],
                    )?;
                    transaction.commit()?;
                    Ok(LeaseReceipt {
                        holder,
                        acquired_at_ms,
                        expires_at_ms: expires_at,
                    })
                },
            )
            .await?
    }

    /// 释放当前持有者的租约。
    ///
    /// 仅当 session 存在且 `holder` 正是当前持有者时删除租约；否则返回
    /// [`SessionStoreError::LeaseNotHeld`]。
    pub async fn release_lease(
        &self,
        session_id: &SessionId,
        holder: impl Into<String>,
    ) -> Result<(), SessionStoreError> {
        let session_id = session_id.to_string();
        let holder = holder.into();
        self.database()
            .call(move |connection| -> Result<(), SessionStoreError> {
                let transaction = connection.transaction()?;
                let exists: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id=?1)",
                    [&session_id],
                    |row| row.get(0),
                )?;
                if !exists {
                    return Err(SessionStoreError::SessionNotFound(session_id));
                }
                let released = transaction.execute(
                    "DELETE FROM session_leases WHERE session_id=?1 AND holder=?2",
                    params![session_id, holder],
                )?;
                if released == 0 {
                    return Err(SessionStoreError::LeaseNotHeld { session_id });
                }
                transaction.commit()?;
                Ok(())
            })
            .await?
    }

    /// 只读检测 session 完整性：parent 缺失与 sequence 间隙。
    ///
    /// 不修改任何数据。正常经 [`SessionStore::append_event`] 写入的 session 始终通过
    /// 检测；该方法用于发现底层损坏（如外键关闭时写入、文件损坏等）。
    pub async fn integrity_check(
        &self,
        session_id: &SessionId,
    ) -> Result<IntegrityReport, SessionStoreError> {
        let session_id = session_id.to_string();
        self.database()
            .call(
                move |connection| -> Result<IntegrityReport, SessionStoreError> {
                    let exists: bool = connection.query_row(
                        "SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id=?1)",
                        [&session_id],
                        |row| row.get(0),
                    )?;
                    if !exists {
                        return Err(SessionStoreError::SessionNotFound(session_id));
                    }

                    let mut statement = connection.prepare(
                        "SELECT sequence FROM session_events \
                     WHERE session_id=?1 ORDER BY sequence ASC",
                    )?;
                    let sequences: Vec<i64> = statement
                        .query_map([&session_id], |row| row.get(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    let event_count = u64::try_from(sequences.len()).unwrap_or(u64::MAX);
                    let max_sequence = sequences.last().copied().unwrap_or(0);

                    let mut sequence_gaps = Vec::new();
                    let mut expected: u64 = 1;
                    for sequence in &sequences {
                        let sequence = u64::try_from(*sequence).unwrap_or(0);
                        if sequence > expected {
                            sequence_gaps.push(SequenceGap {
                                from: expected,
                                to: sequence - 1,
                            });
                        }
                        expected = sequence.saturating_add(1);
                    }

                    let mut statement = connection.prepare(
                        "SELECT e.event_id, e.parent_event_id FROM session_events e \
                     WHERE e.session_id=?1 AND e.parent_event_id IS NOT NULL \
                     AND NOT EXISTS (\
                         SELECT 1 FROM session_events p \
                         WHERE p.session_id=?1 AND p.event_id=e.parent_event_id\
                     )",
                    )?;
                    let missing_parents: Vec<MissingParent> = statement
                        .query_map([&session_id], |row| {
                            Ok(MissingParent {
                                event_id: row.get(0)?,
                                parent_event_id: row.get(1)?,
                            })
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;

                    let ok = sequence_gaps.is_empty() && missing_parents.is_empty();
                    Ok(IntegrityReport {
                        session_id,
                        ok,
                        event_count,
                        max_sequence: u64::try_from(max_sequence).unwrap_or(0),
                        sequence_gaps,
                        missing_parents,
                    })
                },
            )
            .await?
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pawork_domain::{
        AgentEvent, AgentEventEnvelope, EventId, EventSequence, MessageId, RunId, SessionId,
        Timestamp,
    };
    use rusqlite::params;

    use super::*;
    use crate::{SessionStore, DEFAULT_BRANCH_ID};

    fn temp_db() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("lifecycle.sqlite3");
        (dir, path)
    }

    fn event(session: &SessionId, sequence: u64, payload: AgentEvent) -> AgentEventEnvelope {
        AgentEventEnvelope::new(
            EventId::from(format!("event-{sequence}")),
            session.clone(),
            RunId::from("run-1"),
            EventSequence::new(sequence),
            Timestamp::from_unix_millis(1_000 + sequence),
            payload,
        )
    }

    #[tokio::test]
    async fn session_lifecycle_rename_archive_unarchive_resume_delete() {
        let (_dir, path) = temp_db();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-life");
        store
            .create_session(&session, "original", Timestamp::from_unix_millis(1))
            .await
            .expect("session");
        store
            .rename_session(&session, "renamed")
            .await
            .expect("rename");
        store.archive_session(&session).await.expect("archive");
        store.unarchive_session(&session).await.expect("unarchive");

        store
            .acquire_lease(&session, "holder", 60_000)
            .await
            .expect("acquire");
        store.resume_session(&session).await.expect("resume");
        let released = store.release_lease(&session, "holder").await;
        assert!(matches!(
            released,
            Err(SessionStoreError::LeaseNotHeld { .. })
        ));

        store.delete_session(&session).await.expect("delete");
        let missing = store.rename_session(&session, "x").await;
        assert!(matches!(
            missing,
            Err(SessionStoreError::SessionNotFound(_))
        ));

        store.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn lifecycle_ops_error_on_missing_session() {
        let (_dir, path) = temp_db();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let missing = SessionId::from("nope");
        assert!(matches!(
            store.rename_session(&missing, "x").await,
            Err(SessionStoreError::SessionNotFound(_))
        ));
        assert!(matches!(
            store.archive_session(&missing).await,
            Err(SessionStoreError::SessionNotFound(_))
        ));
        assert!(matches!(
            store.unarchive_session(&missing).await,
            Err(SessionStoreError::SessionNotFound(_))
        ));
        assert!(matches!(
            store.resume_session(&missing).await,
            Err(SessionStoreError::SessionNotFound(_))
        ));
        assert!(matches!(
            store.delete_session(&missing).await,
            Err(SessionStoreError::SessionNotFound(_))
        ));
        assert!(matches!(
            store.acquire_lease(&missing, "h", 1_000).await,
            Err(SessionStoreError::SessionNotFound(_))
        ));
        assert!(matches!(
            store.integrity_check(&missing).await,
            Err(SessionStoreError::SessionNotFound(_))
        ));
        store.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn delete_session_with_events_is_rejected() {
        let (_dir, path) = temp_db();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-with-events");
        store
            .create_session(&session, "events", Timestamp::from_unix_millis(1))
            .await
            .expect("session");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    1,
                    AgentEvent::RunStarted {
                        trigger_message_id: MessageId::from("t"),
                    },
                ),
            )
            .await
            .expect("append");
        let blocked = store.delete_session(&session).await;
        assert!(matches!(
            blocked,
            Err(SessionStoreError::SessionHasEvents { .. })
        ));
        store.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn leases_block_concurrent_holders_and_preempt_expired() {
        let (_dir, path) = temp_db();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-lease");
        store
            .create_session(&session, "lease", Timestamp::from_unix_millis(1))
            .await
            .expect("session");

        let receipt = store
            .acquire_lease(&session, "worker-a", 1_000)
            .await
            .expect("acquire");
        assert_eq!(receipt.holder, "worker-a");

        let blocked = store.acquire_lease(&session, "worker-b", 1_000).await;
        assert!(matches!(
            blocked,
            Err(SessionStoreError::LeaseHeld { ref holder, .. }) if holder == "worker-a"
        ));

        let renewed = store
            .renew_lease(&session, "worker-a", 5_000)
            .await
            .expect("renew");
        assert_eq!(renewed.holder, "worker-a");
        assert!(renewed.expires_at_ms >= receipt.expires_at_ms);

        let blocked_renew = store.renew_lease(&session, "worker-b", 5_000).await;
        assert!(matches!(
            blocked_renew,
            Err(SessionStoreError::LeaseHeld { ref holder, .. }) if holder == "worker-a"
        ));

        store
            .release_lease(&session, "worker-a")
            .await
            .expect("release");
        let again = store
            .acquire_lease(&session, "worker-b", 1_000)
            .await
            .expect("acquire b");
        assert_eq!(again.holder, "worker-b");

        let wrong_release = store.release_lease(&session, "worker-a").await;
        assert!(matches!(
            wrong_release,
            Err(SessionStoreError::LeaseNotHeld { .. })
        ));

        store.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn expired_lease_can_be_preempted_on_acquire() {
        let (_dir, path) = temp_db();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-stale");
        store
            .create_session(&session, "stale", Timestamp::from_unix_millis(1))
            .await
            .expect("session");
        let session_id_value = session.to_string();
        store
            .database()
            .call(move |connection| {
                connection.execute(
                    "INSERT INTO session_leases(session_id, holder, acquired_at_ms, expires_at_ms) \
                     VALUES (?1, 'stale', 0, 1)",
                    params![session_id_value],
                )
            })
            .await
            .expect("actor")
            .expect("insert stale lease");
        let preempted = store
            .acquire_lease(&session, "worker-fresh", 1_000)
            .await
            .expect("preempt expired");
        assert_eq!(preempted.holder, "worker-fresh");
        store.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn integrity_check_passes_for_clean_session() {
        let (_dir, path) = temp_db();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-clean");
        store
            .create_session(&session, "clean", Timestamp::from_unix_millis(1))
            .await
            .expect("session");
        for sequence in 1..=3 {
            store
                .append_event(
                    DEFAULT_BRANCH_ID,
                    event(
                        &session,
                        sequence,
                        AgentEvent::CompactionStarted {
                            source_event_count: sequence,
                        },
                    ),
                )
                .await
                .expect("append");
        }
        let report = store.integrity_check(&session).await.expect("integrity");
        assert!(report.ok);
        assert_eq!(report.event_count, 3);
        assert_eq!(report.max_sequence, 3);
        assert!(report.sequence_gaps.is_empty());
        assert!(report.missing_parents.is_empty());
        store.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn integrity_check_detects_gaps_and_missing_parents() {
        let (_dir, path) = temp_db();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-corrupt");
        store
            .create_session(&session, "corrupt", Timestamp::from_unix_millis(1))
            .await
            .expect("session");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    1,
                    AgentEvent::RunStarted {
                        trigger_message_id: MessageId::from("t"),
                    },
                ),
            )
            .await
            .expect("append 1");
        let session_id_value = session.to_string();
        store
            .database()
            .call(move |connection| -> rusqlite::Result<()> {
                connection.execute("PRAGMA foreign_keys=OFF", [])?;
                connection.execute(
                    "INSERT INTO session_events(\
                     event_id, session_id, branch_id, run_id, parent_event_id, \
                     sequence, event_type, schema_version, timestamp_ms, payload_json\
                     ) VALUES ('corrupt', ?1, 'main', 'run-x', 'missing-parent', \
                    5, 'diagnostic', 1, 0, '{}')",
                    params![session_id_value],
                )?;
                Ok(())
            })
            .await
            .expect("actor")
            .expect("corrupt insert");
        let report = store.integrity_check(&session).await.expect("integrity");
        assert!(!report.ok);
        assert!(report
            .sequence_gaps
            .iter()
            .any(|gap| gap.from == 2 && gap.to == 4));
        assert!(report
            .missing_parents
            .iter()
            .any(|missing| missing.event_id == "corrupt"
                && missing.parent_event_id == "missing-parent"));
        store.shutdown().await.expect("shutdown");
    }
}
