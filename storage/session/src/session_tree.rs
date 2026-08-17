//! Session 分支树视图与从任意事件 Fork。
//!
//! [`SessionStore::events_by_branch`] / [`SessionStore::create_branch`] /
//! [`SessionStore::switch_branch`] 已在 `event_store`；本模块的
//! [`SessionStore::fork_from_event`] 复用 `create_branch`，不另写插入。

use pawork_domain::{EventId, SessionId};
use rusqlite::{params, OptionalExtension};

use crate::{SessionStore, SessionStoreError};

/// 树中的一个分支节点。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchNode {
    pub branch_id: String,
    pub parent_branch_id: Option<String>,
    pub forked_from_event_id: Option<String>,
    pub head_sequence: u64,
    /// 是否为当前 session 的 active branch。
    pub active: bool,
}

/// Session 的分支树（扁平节点列表；调用方按 `parent_branch_id` 自行构建树形结构）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionTree {
    pub branches: Vec<BranchNode>,
}

impl SessionStore {
    /// 从 `from_event_id` 指定的事件处 fork 出新 branch。
    ///
    /// 复用 [`SessionStore::create_branch`]；新 branch 的 `parent_branch_id` 取自该
    /// 事件所属的 branch。调用前先校验事件存在、目标 branch 尚不存在，便于给出
    /// 精确的错误变体（`create_branch` 对相同 parent/fork 是幂等成功）。
    pub async fn fork_from_event(
        &self,
        session_id: &SessionId,
        new_branch_id: impl Into<String>,
        from_event_id: &EventId,
    ) -> Result<(), SessionStoreError> {
        let session_id_value = session_id.to_string();
        let new_branch_id = new_branch_id.into();
        let from_event_id = from_event_id.to_string();

        let lookup_session_id = session_id_value.clone();
        let lookup_branch_id = new_branch_id.clone();
        let lookup_event_id = from_event_id.clone();
        let parent_branch_id = self
            .database()
            .call(move |connection| -> Result<String, SessionStoreError> {
                let parent_branch_id: Option<String> = connection
                    .query_row(
                        "SELECT branch_id FROM session_events \
                         WHERE session_id=?1 AND event_id=?2",
                        params![lookup_session_id, lookup_event_id.clone()],
                        |row| row.get(0),
                    )
                    .optional()?;
                let parent_branch_id = parent_branch_id
                    .ok_or_else(|| SessionStoreError::ParentEventNotFound(lookup_event_id))?;
                let branch_exists: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM session_branches \
                     WHERE session_id=?1 AND branch_id=?2)",
                    params![lookup_session_id, lookup_branch_id.clone()],
                    |row| row.get(0),
                )?;
                if branch_exists {
                    return Err(SessionStoreError::BranchAlreadyExists {
                        session_id: lookup_session_id,
                        branch_id: lookup_branch_id,
                    });
                }
                Ok(parent_branch_id)
            })
            .await??;

        let session_id = SessionId::from(session_id_value);
        self.create_branch(
            &session_id,
            new_branch_id,
            Some(parent_branch_id),
            Some(from_event_id),
        )
        .await
    }

    /// 返回 session 的分支树视图。
    pub async fn session_tree(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionTree, SessionStoreError> {
        let session_id = session_id.to_string();
        let query_session_id = session_id.clone();
        let branches = self
            .database()
            .call(
                move |connection| -> Result<Vec<BranchNode>, SessionStoreError> {
                    let mut statement = connection.prepare(
                        "SELECT b.branch_id, b.parent_branch_id, b.forked_from_event_id, \
                     b.head_sequence, s.active_branch = b.branch_id AS active \
                     FROM session_branches b \
                     JOIN sessions s ON s.session_id = b.session_id \
                     WHERE b.session_id=?1 \
                     ORDER BY b.branch_id",
                    )?;
                    let rows = statement
                        .query_map([&query_session_id], |row| {
                            let head_sequence: i64 = row.get(3)?;
                            let active: bool = row.get(4)?;
                            Ok(BranchNode {
                                branch_id: row.get(0)?,
                                parent_branch_id: row.get(1)?,
                                forked_from_event_id: row.get(2)?,
                                head_sequence: u64::try_from(head_sequence).unwrap_or(0),
                                active,
                            })
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    Ok(rows)
                },
            )
            .await??;
        if branches.is_empty() {
            return Err(SessionStoreError::SessionNotFound(session_id));
        }
        Ok(SessionTree { branches })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pawork_domain::{
        AgentEvent, AgentEventEnvelope, EventId, EventSequence, MessageId, RunId, SessionId,
        Timestamp,
    };

    use super::*;
    use crate::{SessionStore, DEFAULT_BRANCH_ID};

    fn temp_db() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session-tree.sqlite3");
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
    async fn fork_from_event_creates_child_branch_pointing_at_event() {
        let (_dir, path) = temp_db();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-fork");
        store
            .create_session(&session, "fork", Timestamp::from_unix_millis(1))
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
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(&session, 2, AgentEvent::RunCancelled { reason: None }),
            )
            .await
            .expect("append 2");

        store
            .fork_from_event(&session, "experiment", &EventId::from("event-2"))
            .await
            .expect("fork");

        let tree = store.session_tree(&session).await.expect("tree");
        assert_eq!(tree.branches.len(), 2);
        let main = tree
            .branches
            .iter()
            .find(|node| node.branch_id == DEFAULT_BRANCH_ID)
            .expect("main branch");
        assert!(main.active);
        assert_eq!(main.head_sequence, 2);
        let experiment = tree
            .branches
            .iter()
            .find(|node| node.branch_id == "experiment")
            .expect("experiment branch");
        assert!(!experiment.active);
        assert_eq!(
            experiment.parent_branch_id.as_deref(),
            Some(DEFAULT_BRANCH_ID)
        );
        assert_eq!(experiment.forked_from_event_id.as_deref(), Some("event-2"));
        assert_eq!(experiment.head_sequence, 2);

        let main_events = store
            .events_by_branch(&session, DEFAULT_BRANCH_ID, 1, 10)
            .await
            .expect("main events");
        assert_eq!(
            main_events
                .iter()
                .map(|envelope| envelope.sequence.value())
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        let experiment_events = store
            .events_by_branch(&session, "experiment", 1, 10)
            .await
            .expect("experiment events");
        assert!(experiment_events.is_empty());

        store.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn fork_from_event_rejects_missing_event_and_duplicate_branch() {
        let (_dir, path) = temp_db();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-fork-err");
        store
            .create_session(&session, "fork-err", Timestamp::from_unix_millis(1))
            .await
            .expect("session");
        let missing = store
            .fork_from_event(&session, "branch-a", &EventId::from("nope"))
            .await;
        assert!(matches!(
            missing,
            Err(SessionStoreError::ParentEventNotFound(_))
        ));

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
        store
            .fork_from_event(&session, "branch-a", &EventId::from("event-1"))
            .await
            .expect("first fork");
        let duplicate = store
            .fork_from_event(&session, "branch-a", &EventId::from("event-1"))
            .await;
        assert!(matches!(
            duplicate,
            Err(SessionStoreError::BranchAlreadyExists { ref branch_id, .. })
                if branch_id == "branch-a"
        ));

        store.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn session_tree_errors_when_session_missing() {
        let (_dir, path) = temp_db();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let missing = store.session_tree(&SessionId::from("nope")).await;
        assert!(matches!(
            missing,
            Err(SessionStoreError::SessionNotFound(_))
        ));
        store.shutdown().await.expect("shutdown");
    }
}
