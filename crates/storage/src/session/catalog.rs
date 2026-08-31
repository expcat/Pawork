use pawork_domain::{SessionId, WorkspaceId};
use rusqlite::{params, OptionalExtension};

use crate::session::{SessionStore, SessionStoreError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRecord {
    pub session_id: String,
    pub title: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub archived: bool,
    pub active_branch: String,
    /// ADR-043（schema v13）：Session→Workspace 归属，弱引用列；
    /// 历史会话（迁移前创建）为 `None`，消费方按 Unassigned 处理。
    pub workspace_id: Option<String>,
}

impl SessionStore {
    /// 列出全部已持久化的 Session→Workspace 归属，包括归档会话。
    pub async fn list_session_workspace_bindings(
        &self,
    ) -> Result<Vec<(SessionId, WorkspaceId)>, SessionStoreError> {
        let rows = self
            .database()
            .call(|connection| -> rusqlite::Result<Vec<(String, String)>> {
                let mut statement = connection.prepare(
                    "SELECT session_id, workspace_id FROM sessions WHERE workspace_id IS NOT NULL ORDER BY session_id",
                )?;
                let rows = statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .await??;
        Ok(rows
            .into_iter()
            .map(|(session_id, workspace_id)| {
                (SessionId::from(session_id), WorkspaceId::from(workspace_id))
            })
            .collect())
    }

    /// 列出未归档会话，按 `updated_at_ms` 降序。
    pub async fn list_sessions(&self) -> Result<Vec<SessionRecord>, SessionStoreError> {
        let rows = self
            .database()
            .call(|connection| -> rusqlite::Result<Vec<(String, String, i64, i64, i64, String, Option<String>)>> {
                let mut statement = connection.prepare(
                    "SELECT session_id, title, created_at_ms, updated_at_ms, archived, active_branch, workspace_id \
                     FROM sessions WHERE archived=0 ORDER BY updated_at_ms DESC",
                )?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .await??;
        Ok(rows
            .into_iter()
            .map(
                |(
                    session_id,
                    title,
                    created_at_ms,
                    updated_at_ms,
                    archived,
                    active_branch,
                    workspace_id,
                )| {
                    SessionRecord {
                        session_id,
                        title,
                        created_at_ms,
                        updated_at_ms,
                        archived: archived != 0,
                        active_branch,
                        workspace_id,
                    }
                },
            )
            .collect())
    }

    pub async fn get_session(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionRecord, SessionStoreError> {
        let session_id = session_id.to_string();
        let lookup = session_id.clone();
        let row = self
            .database()
            .call(move |connection| -> rusqlite::Result<Option<(String, String, i64, i64, i64, String, Option<String>)>> {
                connection
                    .query_row(
                        "SELECT session_id, title, created_at_ms, updated_at_ms, archived, active_branch, workspace_id \
                         FROM sessions WHERE session_id=?1",
                        params![lookup],
                        |row| {
                            Ok((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                row.get(3)?,
                                row.get(4)?,
                                row.get(5)?,
                                row.get(6)?,
                            ))
                        },
                    )
                    .optional()
            })
            .await??;
        let Some((
            session_id,
            title,
            created_at_ms,
            updated_at_ms,
            archived,
            active_branch,
            workspace_id,
        )) = row
        else {
            return Err(SessionStoreError::SessionNotFound(session_id));
        };
        Ok(SessionRecord {
            session_id,
            title,
            created_at_ms,
            updated_at_ms,
            archived: archived != 0,
            active_branch,
            workspace_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pawork_domain::{SessionId, Timestamp};

    use crate::session::{SessionStore, SessionStoreError};

    fn temp_db() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("catalog.sqlite3");
        (dir, path)
    }

    #[tokio::test]
    async fn list_sessions_orders_by_updated_at_and_hides_archived() {
        let (_dir, path) = temp_db();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        store
            .create_session(
                &SessionId::from("session-a"),
                "alpha",
                Timestamp::from_unix_millis(100),
            )
            .await
            .expect("a");
        store
            .create_session(
                &SessionId::from("session-b"),
                "bravo",
                Timestamp::from_unix_millis(300),
            )
            .await
            .expect("b");
        store
            .create_session(
                &SessionId::from("session-c"),
                "charlie",
                Timestamp::from_unix_millis(200),
            )
            .await
            .expect("c");
        store
            .database()
            .call(|connection| {
                connection.execute(
                    "UPDATE sessions SET archived=1 WHERE session_id='session-c'",
                    [],
                )
            })
            .await
            .expect("actor")
            .expect("archive");

        let listed = store.list_sessions().await.expect("list");
        assert_eq!(
            listed
                .iter()
                .map(|record| record.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["session-b", "session-a"]
        );
        assert_eq!(listed[0].updated_at_ms, 300);
        assert_eq!(listed[1].updated_at_ms, 100);
        assert!(listed.iter().all(|record| !record.archived));

        let fetched = store
            .get_session(&SessionId::from("session-b"))
            .await
            .expect("get");
        assert_eq!(fetched.title, "bravo");
        assert_eq!(fetched.active_branch, "main");

        store.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn get_session_missing_returns_not_found() {
        let (_dir, path) = temp_db();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let error = store
            .get_session(&SessionId::from("missing"))
            .await
            .expect_err("missing");
        assert!(matches!(
            error,
            SessionStoreError::SessionNotFound(ref id) if id == "missing"
        ));
        store.shutdown().await.expect("shutdown");
    }
}
