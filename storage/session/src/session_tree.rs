//! Session 分支树视图、祖先链与从任意事件 Fork。
//!
//! [`SessionStore::events_by_branch`] / [`SessionStore::create_branch`] /
//! [`SessionStore::switch_branch`] 已在 `event_store`；本模块的
//! [`SessionStore::fork_from_event`] 复用 `create_branch`，不另写插入。
//! [`SessionStore::events_by_branch`] 只含本支追加，不能当 resume 源——
//! resume / compact / Timeline 必须走 [`SessionStore::events_on_lineage`]。

use std::collections::HashSet;

use pawork_domain::{AgentEventEnvelope, EventId, SessionId};
use rusqlite::{params, Connection, OptionalExtension};

use crate::{SessionStore, SessionStoreError};

/// 本支无上界；祖先支的 `max_sequence` 含 fork 点事件本身。
pub(crate) const LINEAGE_UNBOUNDED: i64 = i64::MAX;

/// 沿 `parent_branch_id` + `forked_from_event_id` 走到 root。
/// 本支：`max_sequence = i64::MAX`；祖先支：fork 点事件的 sequence（含该事件）。
pub(crate) fn load_ancestor_lineage(
    connection: &Connection,
    session_id: &str,
    branch_id: &str,
) -> Result<Vec<(String, i64)>, SessionStoreError> {
    let session_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id=?1)",
        [session_id],
        |row| row.get(0),
    )?;
    if !session_exists {
        return Err(SessionStoreError::SessionNotFound(session_id.into()));
    }

    let mut lineage = Vec::new();
    let mut current = branch_id.to_string();
    let mut seen = HashSet::new();
    let mut is_tip = true;

    loop {
        if !seen.insert(current.clone()) {
            return Err(SessionStoreError::ProjectionInvariant(format!(
                "branch lineage cycle at {current}"
            )));
        }
        let row: Option<(Option<String>, Option<String>)> = connection
            .query_row(
                "SELECT parent_branch_id, forked_from_event_id \
                 FROM session_branches WHERE session_id=?1 AND branch_id=?2",
                params![session_id, current],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((parent_branch_id, forked_from_event_id)) = row else {
            return Err(SessionStoreError::BranchNotFound {
                session_id: session_id.into(),
                branch_id: current,
            });
        };

        if is_tip {
            lineage.push((current.clone(), LINEAGE_UNBOUNDED));
            is_tip = false;
        }

        let Some(parent) = parent_branch_id.filter(|value| !value.is_empty()) else {
            break;
        };
        let max_sequence = match forked_from_event_id.as_deref() {
            Some(event_id) => connection
                .query_row(
                    "SELECT sequence FROM session_events \
                     WHERE session_id=?1 AND event_id=?2",
                    params![session_id, event_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .ok_or_else(|| SessionStoreError::ParentEventNotFound(event_id.into()))?,
            None => 0,
        };
        lineage.push((parent.clone(), max_sequence));
        current = parent;
    }

    Ok(lineage)
}

pub(crate) fn visible_on_lineage(lineage: &[(String, i64)], branch_id: &str, sequence: i64) -> bool {
    lineage
        .iter()
        .any(|(bound_branch, max_sequence)| bound_branch == branch_id && sequence <= *max_sequence)
}

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

    /// 从 `branch_id` 沿 parent / fork 点走到 root。
    ///
    /// 本支 `max_sequence` 为 [`LINEAGE_UNBOUNDED`]；祖先支为该 fork 点事件的
    /// sequence（含该事件）。
    pub async fn ancestor_lineage(
        &self,
        session_id: &SessionId,
        branch_id: impl Into<String>,
    ) -> Result<Vec<(String, i64)>, SessionStoreError> {
        let session_id = session_id.to_string();
        let branch_id = branch_id.into();
        self.database()
            .call(move |connection| load_ancestor_lineage(connection, &session_id, &branch_id))
            .await?
    }

    /// 祖先前缀 ∪ 本支追加，按全局 sequence 升序。
    ///
    /// [`SessionStore::events_by_branch`] 只含本支追加，不能当 resume 源。
    pub async fn events_on_lineage(
        &self,
        session_id: &SessionId,
        branch_id: impl Into<String>,
        from_sequence: u64,
        limit: usize,
    ) -> Result<Vec<AgentEventEnvelope>, SessionStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let session_id = session_id.to_string();
        let branch_id = branch_id.into();
        let from_sequence =
            i64::try_from(from_sequence).map_err(|_| SessionStoreError::SequenceOverflow)?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let json_rows = self
            .database()
            .call(move |connection| -> Result<Vec<String>, SessionStoreError> {
                let lineage = load_ancestor_lineage(connection, &session_id, &branch_id)?;
                let mut statement = connection.prepare(
                    "SELECT payload_json, branch_id, sequence FROM session_events \
                     WHERE session_id=?1 AND sequence>=?2 ORDER BY sequence ASC",
                )?;
                let rows = statement
                    .query_map(params![session_id, from_sequence], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows
                    .into_iter()
                    .filter(|(_, event_branch, sequence)| {
                        visible_on_lineage(&lineage, event_branch, *sequence)
                    })
                    .take(usize::try_from(limit).unwrap_or(usize::MAX))
                    .map(|(json, _, _)| json)
                    .collect())
            })
            .await??;
        json_rows
            .into_iter()
            .map(|json| serde_json::from_str(&json).map_err(SessionStoreError::from))
            .collect()
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

    #[tokio::test]
    async fn ancestor_lineage_and_events_exclude_post_fork_parent_appends() {
        let (_dir, path) = temp_db();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-lineage");
        store
            .create_session(&session, "lineage", Timestamp::from_unix_millis(1))
            .await
            .expect("session");
        for sequence in 1..=3u64 {
            store
                .append_event(
                    DEFAULT_BRANCH_ID,
                    event(
                        &session,
                        sequence,
                        AgentEvent::MessageCommitted {
                            message: pawork_domain::Message {
                                id: pawork_domain::MessageId::from(format!("m-{sequence}")),
                                role: pawork_domain::MessageRole::User,
                                content: vec![pawork_domain::ContentPart::Text(
                                    pawork_domain::TextContent {
                                        text: format!("msg-{sequence}"),
                                    },
                                )],
                                metadata: Default::default(),
                            },
                        },
                    ),
                )
                .await
                .expect("append");
        }
        store
            .fork_from_event(&session, "experiment", &EventId::from("event-1"))
            .await
            .expect("fork");
        store
            .switch_branch(&session, "experiment")
            .await
            .expect("switch");

        let lineage = store
            .ancestor_lineage(&session, "experiment")
            .await
            .expect("lineage");
        assert_eq!(
            lineage,
            vec![
                ("experiment".into(), LINEAGE_UNBOUNDED),
                (DEFAULT_BRANCH_ID.into(), 1),
            ]
        );

        let sequences = |events: Vec<AgentEventEnvelope>| {
            events
                .into_iter()
                .map(|envelope| envelope.sequence.value())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            sequences(
                store
                    .events_on_lineage(&session, "experiment", 1, 10)
                    .await
                    .expect("lineage events")
            ),
            vec![1],
            "fork lineage 只含祖先前缀，不含 main 在 fork 点之后的 2–3"
        );
        assert_eq!(
            sequences(
                store
                    .events_by_branch(&session, "experiment", 1, 10)
                    .await
                    .expect("branch-only")
            ),
            Vec::<u64>::new(),
            "events_by_branch 只含本支追加，不能当 resume 源"
        );
        assert_eq!(
            sequences(
                store
                    .events_on_lineage(&session, DEFAULT_BRANCH_ID, 1, 10)
                    .await
                    .expect("main lineage")
            ),
            vec![1, 2, 3]
        );

        let snapshot = store
            .projection_snapshot(&session)
            .await
            .expect("active snapshot");
        let ids: Vec<&str> = snapshot
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect();
        assert_eq!(ids, vec!["m-1"]);

        store.shutdown().await.expect("shutdown");
    }
}
