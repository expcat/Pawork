//! 命令幂等账本（storage v11 `command_ledger`）。
//!
//! 作用域为 `(tenant_id, client_scope, command_id)`，可选 `idempotency_key`
//! 在同一作用域内唯一。持久态以 SQLite 为准；storage 只存 opaque JSON 字符串。

use rusqlite::{params, Connection, ErrorCode, OptionalExtension};
use thiserror::Error;

use crate::session::{SessionStore, SessionStoreError};
use crate::sqlite::{DatabaseActor, DatabaseError};

pub const DEFAULT_COMMAND_LEDGER_CAPACITY: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LedgerCheck {
    New,
    Replay(String),
    InFlight,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LedgerStats {
    pub entries: usize,
    pub inflight: usize,
    pub completed: usize,
}

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("command {0} was already recorded")]
    DuplicateCommand(String),
    #[error("idempotency key {key} is already bound to command {existing}")]
    KeyConflict { key: String, existing: String },
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

#[derive(Clone)]
pub struct CommandLedger {
    database: DatabaseActor,
}

impl CommandLedger {
    pub fn new(database: DatabaseActor) -> Self {
        Self { database }
    }

    pub async fn check(
        &self,
        tenant_id: &str,
        client_scope: &str,
        command_id: &str,
        idempotency_key: Option<&str>,
    ) -> Result<LedgerCheck, LedgerError> {
        let tenant_id = tenant_id.to_string();
        let client_scope = client_scope.to_string();
        let command_id = command_id.to_string();
        let idempotency_key = idempotency_key.map(str::to_string);
        let created_at_ms = now_ms();
        Ok(self
            .database
            .call(move |connection| {
                check_and_reserve(
                    connection,
                    &tenant_id,
                    &client_scope,
                    &command_id,
                    idempotency_key.as_deref(),
                    created_at_ms,
                )
            })
            .await??)
    }

    pub async fn record(
        &self,
        tenant_id: &str,
        client_scope: &str,
        command_id: &str,
        idempotency_key: Option<&str>,
        response_json: &str,
        capacity: usize,
    ) -> Result<(), LedgerError> {
        let tenant_id = tenant_id.to_string();
        let client_scope = client_scope.to_string();
        let command_id = command_id.to_string();
        let idempotency_key = idempotency_key.map(str::to_string);
        let response_json = response_json.to_string();
        let completed_at_ms = now_ms();
        self.database
            .call(move |connection| {
                record_completed(
                    connection,
                    &tenant_id,
                    &client_scope,
                    &command_id,
                    idempotency_key.as_deref(),
                    &response_json,
                    completed_at_ms,
                    capacity.max(1),
                )
            })
            .await??;
        Ok(())
    }

    pub async fn release(
        &self,
        tenant_id: &str,
        client_scope: &str,
        command_id: &str,
        idempotency_key: Option<&str>,
    ) -> Result<(), LedgerError> {
        let tenant_id = tenant_id.to_string();
        let client_scope = client_scope.to_string();
        let command_id = command_id.to_string();
        let idempotency_key = idempotency_key.map(str::to_string);
        self.database
            .call(move |connection| {
                release_inflight(
                    connection,
                    &tenant_id,
                    &client_scope,
                    &command_id,
                    idempotency_key.as_deref(),
                )
            })
            .await??;
        Ok(())
    }

    /// 单宿主进程模型：open 写库后回收上次崩溃遗留的 inflight 占位。
    /// `open_read_only` 不执行。
    pub async fn reclaim_inflight(&self) -> Result<u64, LedgerError> {
        let deleted = self
            .database
            .call(|connection| {
                connection.execute("DELETE FROM command_ledger WHERE status='inflight'", [])
            })
            .await??;
        Ok(deleted as u64)
    }

    pub async fn stats(&self) -> Result<LedgerStats, LedgerError> {
        Ok(self
            .database
            .call(|connection| -> Result<LedgerStats, rusqlite::Error> {
                let entries: i64 =
                    connection
                        .query_row("SELECT COUNT(*) FROM command_ledger", [], |row| row.get(0))?;
                let inflight: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM command_ledger WHERE status='inflight'",
                    [],
                    |row| row.get(0),
                )?;
                let completed: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM command_ledger WHERE status='completed'",
                    [],
                    |row| row.get(0),
                )?;
                Ok(LedgerStats {
                    entries: entries.max(0) as usize,
                    inflight: inflight.max(0) as usize,
                    completed: completed.max(0) as usize,
                })
            })
            .await??)
    }
}

impl SessionStore {
    pub fn command_ledger(&self) -> CommandLedger {
        CommandLedger::new(self.database().clone())
    }

    pub async fn waiting_tool_call(
        &self,
        tool_call_id: &str,
    ) -> Result<Option<WaitingToolCall>, SessionStoreError> {
        let tool_call_id = tool_call_id.to_string();
        Ok(self
            .database()
            .call(move |connection| load_waiting_tool_call(connection, &tool_call_id))
            .await??)
    }

    pub async fn waiting_tool_calls(&self) -> Result<Vec<WaitingToolCall>, SessionStoreError> {
        Ok(self
            .database()
            .call(|connection| load_waiting_tool_calls(connection))
            .await??)
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Clone)]
struct LedgerRow {
    command_id: String,
    status: String,
    response_json: Option<String>,
}

fn lookup_row(
    connection: &mut Connection,
    tenant_id: &str,
    client_scope: &str,
    command_id: Option<&str>,
    idempotency_key: Option<&str>,
) -> rusqlite::Result<Option<LedgerRow>> {
    if let Some(command_id) = command_id {
        if let Some(row) = connection
            .query_row(
                "SELECT command_id, status, response_json FROM command_ledger                  WHERE tenant_id=?1 AND client_scope=?2 AND command_id=?3",
                params![tenant_id, client_scope, command_id],
                |row| {
                    Ok(LedgerRow {
                        command_id: row.get(0)?,
                        status: row.get(1)?,
                        response_json: row.get(2)?,
                    })
                },
            )
            .optional()?
        {
            return Ok(Some(row));
        }
    }
    if let Some(key) = idempotency_key {
        return connection
            .query_row(
                "SELECT command_id, status, response_json FROM command_ledger                  WHERE tenant_id=?1 AND client_scope=?2 AND idempotency_key=?3",
                params![tenant_id, client_scope, key],
                |row| {
                    Ok(LedgerRow {
                        command_id: row.get(0)?,
                        status: row.get(1)?,
                        response_json: row.get(2)?,
                    })
                },
            )
            .optional();
    }
    Ok(None)
}

fn row_decision(row: &LedgerRow) -> LedgerCheck {
    if row.status == "completed" {
        LedgerCheck::Replay(row.response_json.clone().unwrap_or_default())
    } else {
        LedgerCheck::InFlight
    }
}

fn check_and_reserve(
    connection: &mut Connection,
    tenant_id: &str,
    client_scope: &str,
    command_id: &str,
    idempotency_key: Option<&str>,
    created_at_ms: i64,
) -> Result<LedgerCheck, LedgerError> {
    if let Some(row) = lookup_row(connection, tenant_id, client_scope, Some(command_id), None)? {
        return Ok(row_decision(&row));
    }
    if let Some(row) = lookup_row(connection, tenant_id, client_scope, None, idempotency_key)? {
        return Ok(row_decision(&row));
    }
    match connection.execute(
        "INSERT INTO command_ledger(             tenant_id, client_scope, command_id, idempotency_key, status, created_at_ms         ) VALUES (?1, ?2, ?3, ?4, 'inflight', ?5)",
        params![
            tenant_id,
            client_scope,
            command_id,
            idempotency_key,
            created_at_ms
        ],
    ) {
        Ok(_) => Ok(LedgerCheck::New),
        Err(error) if is_unique_violation(&error) => {
            if let Some(row) = lookup_row(
                connection,
                tenant_id,
                client_scope,
                Some(command_id),
                idempotency_key,
            )? {
                Ok(row_decision(&row))
            } else {
                Err(LedgerError::Sqlite(error))
            }
        }
        Err(error) => Err(LedgerError::Sqlite(error)),
    }
}

fn record_completed(
    connection: &mut Connection,
    tenant_id: &str,
    client_scope: &str,
    command_id: &str,
    idempotency_key: Option<&str>,
    response_json: &str,
    completed_at_ms: i64,
    capacity: usize,
) -> Result<(), LedgerError> {
    if let Some(row) = lookup_row(connection, tenant_id, client_scope, Some(command_id), None)? {
        if row.status == "completed" {
            return Err(LedgerError::DuplicateCommand(command_id.to_string()));
        }
    }
    if let Some(key) = idempotency_key {
        if let Some(existing) = lookup_row(connection, tenant_id, client_scope, None, Some(key))? {
            if existing.command_id != command_id {
                return Err(LedgerError::KeyConflict {
                    key: key.to_string(),
                    existing: existing.command_id,
                });
            }
        }
    }
    let updated = match connection.execute(
        "UPDATE command_ledger SET status='completed', response_json=?1, completed_at_ms=?2,          idempotency_key=COALESCE(idempotency_key, ?3)          WHERE tenant_id=?4 AND client_scope=?5 AND command_id=?6 AND status='inflight'",
        params![
            response_json,
            completed_at_ms,
            idempotency_key,
            tenant_id,
            client_scope,
            command_id
        ],
    ) {
        Ok(count) => count,
        Err(error) if is_unique_violation(&error) => {
            let existing = lookup_row(connection, tenant_id, client_scope, None, idempotency_key)?
                .map(|row| row.command_id)
                .unwrap_or_default();
            return Err(LedgerError::KeyConflict {
                key: idempotency_key.unwrap_or_default().to_string(),
                existing,
            });
        }
        Err(error) => return Err(LedgerError::Sqlite(error)),
    };
    if updated == 0 {
        match connection.execute(
            "INSERT INTO command_ledger(                 tenant_id, client_scope, command_id, idempotency_key, status,                  response_json, created_at_ms, completed_at_ms             ) VALUES (?1, ?2, ?3, ?4, 'completed', ?5, ?6, ?6)",
            params![
                tenant_id,
                client_scope,
                command_id,
                idempotency_key,
                response_json,
                completed_at_ms
            ],
        ) {
            Ok(_) => {}
            Err(error) if is_unique_violation(&error) => {
                if lookup_row(connection, tenant_id, client_scope, Some(command_id), None)?
                    .is_some_and(|row| row.status == "completed")
                {
                    return Err(LedgerError::DuplicateCommand(command_id.to_string()));
                }
                let existing =
                    lookup_row(connection, tenant_id, client_scope, None, idempotency_key)?
                        .map(|row| row.command_id)
                        .unwrap_or_default();
                return Err(LedgerError::KeyConflict {
                    key: idempotency_key.unwrap_or_default().to_string(),
                    existing,
                });
            }
            Err(error) => return Err(LedgerError::Sqlite(error)),
        }
    }
    evict_completed(connection, capacity)?;
    Ok(())
}

fn release_inflight(
    connection: &mut Connection,
    tenant_id: &str,
    client_scope: &str,
    command_id: &str,
    idempotency_key: Option<&str>,
) -> rusqlite::Result<()> {
    let deleted = connection.execute(
        "DELETE FROM command_ledger WHERE tenant_id=?1 AND client_scope=?2 AND command_id=?3 AND status='inflight'",
        params![tenant_id, client_scope, command_id],
    )?;
    if deleted == 0 {
        if let Some(key) = idempotency_key {
            connection.execute(
                "DELETE FROM command_ledger WHERE tenant_id=?1 AND client_scope=?2 AND idempotency_key=?3 AND status='inflight'",
                params![tenant_id, client_scope, key],
            )?;
        }
    }
    Ok(())
}

fn evict_completed(connection: &mut Connection, capacity: usize) -> rusqlite::Result<()> {
    // Capacity is global across tenant_id and client_scope, matching the in-memory
    // predecessor (DEFAULT_COMMAND_LEDGER_CAPACITY completed rows, oldest first).
    let completed: i64 = connection.query_row(
        "SELECT COUNT(*) FROM command_ledger WHERE status='completed'",
        [],
        |row| row.get(0),
    )?;
    let overflow = completed.saturating_sub(capacity as i64);
    if overflow <= 0 {
        return Ok(());
    }
    connection.execute(
        "DELETE FROM command_ledger WHERE rowid IN (             SELECT rowid FROM command_ledger WHERE status='completed'              ORDER BY completed_at_ms ASC, created_at_ms ASC LIMIT ?1         )",
        params![overflow],
    )?;
    Ok(())
}

fn is_unique_violation(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::ConstraintViolation)
    )
}

#[derive(Clone, Debug, PartialEq)]
pub struct WaitingToolCall {
    pub session_id: pawork_domain::SessionId,
    pub tool_call: crate::session::ProjectedToolCall,
}

fn map_waiting_row(
    session_id: String,
    tool_call_id: String,
    run_id: String,
    name: String,
    state: String,
    arguments_json: String,
    result: Option<String>,
) -> Result<WaitingToolCall, SessionStoreError> {
    Ok(WaitingToolCall {
        session_id: pawork_domain::SessionId::from(session_id),
        tool_call: crate::session::ProjectedToolCall {
            tool_call_id: pawork_domain::ToolCallId::from(tool_call_id),
            run_id: pawork_domain::RunId::from(run_id),
            name,
            state,
            arguments_json,
            result: result.map(|json| serde_json::from_str(&json)).transpose()?,
        },
    })
}

fn load_waiting_tool_call(
    connection: &mut Connection,
    tool_call_id: &str,
) -> Result<Option<WaitingToolCall>, SessionStoreError> {
    let row = connection
        .query_row(
            "SELECT session_id, tool_call_id, run_id, name, state, arguments_json, result_json              FROM tool_calls WHERE tool_call_id=?1 AND state='waiting_for_approval'",
            [tool_call_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(session_id, tool_call_id, run_id, name, state, arguments_json, result)| {
            map_waiting_row(
                session_id,
                tool_call_id,
                run_id,
                name,
                state,
                arguments_json,
                result,
            )
        },
    )
    .transpose()
}

fn load_waiting_tool_calls(
    connection: &mut Connection,
) -> Result<Vec<WaitingToolCall>, SessionStoreError> {
    let mut statement = connection.prepare(
        "SELECT session_id, tool_call_id, run_id, name, state, arguments_json, result_json          FROM tool_calls WHERE state='waiting_for_approval' ORDER BY tool_call_id",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(
            |(session_id, tool_call_id, run_id, name, state, arguments_json, result)| {
                map_waiting_row(
                    session_id,
                    tool_call_id,
                    run_id,
                    name,
                    state,
                    arguments_json,
                    result,
                )
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionStore;

    #[tokio::test]
    async fn new_record_replay_survives_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ledger.sqlite3");
        let (store, _) = SessionStore::open(&path).await.expect("open");
        let ledger = store.command_ledger();
        let check = ledger
            .check("tenant-a", "gui-1", "cmd-1", Some("key-1"))
            .await
            .expect("check");
        assert_eq!(check, LedgerCheck::New);
        ledger
            .record(
                "tenant-a",
                "gui-1",
                "cmd-1",
                Some("key-1"),
                r#"{"ok":true}"#,
                4096,
            )
            .await
            .expect("record");
        match ledger
            .check("tenant-a", "gui-1", "cmd-2", Some("key-1"))
            .await
            .expect("key replay")
        {
            LedgerCheck::Replay(json) => assert_eq!(json, r#"{"ok":true}"#),
            other => panic!("expected replay, got {other:?}"),
        }
        store.shutdown().await.expect("shutdown");

        let (store, _) = SessionStore::open(&path).await.expect("reopen");
        match store
            .command_ledger()
            .check("tenant-a", "gui-1", "cmd-1", Some("key-1"))
            .await
            .expect("restart replay")
        {
            LedgerCheck::Replay(json) => assert_eq!(json, r#"{"ok":true}"#),
            other => panic!("expected replay after reopen, got {other:?}"),
        }
        store.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn key_conflict_release_reclaim_and_capacity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ledger.sqlite3");
        let (store, _) = SessionStore::open(&path).await.expect("open");
        let ledger = store.command_ledger();
        ledger
            .record("t", "s", "cmd-1", Some("key-1"), "one", 2)
            .await
            .expect("record 1");
        let conflict = ledger
            .record("t", "s", "cmd-2", Some("key-1"), "two", 2)
            .await
            .expect_err("key conflict");
        assert!(matches!(conflict, LedgerError::KeyConflict { .. }));

        assert_eq!(
            ledger.check("t", "s", "cmd-3", None).await.expect("new"),
            LedgerCheck::New
        );
        ledger
            .release("t", "s", "cmd-3", None)
            .await
            .expect("release");
        assert_eq!(
            ledger
                .check("t", "s", "cmd-3", None)
                .await
                .expect("after release"),
            LedgerCheck::New
        );
        ledger
            .release("t", "s", "cmd-3", None)
            .await
            .expect("release unused");

        ledger
            .record("t", "s", "cmd-2", Some("key-2"), "two", 2)
            .await
            .expect("record 2");
        ledger
            .record("t", "s", "cmd-4", None, "four", 2)
            .await
            .expect("record 4 evicts oldest");
        let stats = ledger.stats().await.expect("stats");
        assert_eq!(stats.completed, 2);
        assert_eq!(
            ledger
                .check("t", "s", "cmd-1", Some("key-1"))
                .await
                .expect("evicted"),
            LedgerCheck::New
        );
        ledger
            .release("t", "s", "cmd-1", Some("key-1"))
            .await
            .expect("release evicted");
        match ledger.check("t", "s", "cmd-4", None).await.expect("kept") {
            LedgerCheck::Replay(json) => assert_eq!(json, "four"),
            other => panic!("expected replay, got {other:?}"),
        }

        assert_eq!(
            ledger
                .check("t", "s", "cmd-inflight", None)
                .await
                .expect("inflight"),
            LedgerCheck::New
        );
        drop(store);
        let (store, _) = SessionStore::open(&path).await.expect("reopen reclaim");
        assert_eq!(
            store
                .command_ledger()
                .check("t", "s", "cmd-inflight", None)
                .await
                .expect("reclaimed"),
            LedgerCheck::New
        );
        store.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn eviction_is_global_across_tenant_and_scope() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ledger.sqlite3");
        let (store, _) = SessionStore::open(&path).await.expect("open");
        let ledger = store.command_ledger();
        ledger
            .record("tenant-a", "gui-1", "cmd-a", Some("key-a"), "a", 2)
            .await
            .expect("record a");
        ledger
            .record("tenant-b", "gui-2", "cmd-b", Some("key-b"), "b", 2)
            .await
            .expect("record b");
        ledger
            .record("tenant-c", "cli", "cmd-c", None, "c", 2)
            .await
            .expect("record c evicts oldest globally");
        let stats = ledger.stats().await.expect("stats");
        assert_eq!(stats.completed, 2);
        assert_eq!(
            ledger
                .check("tenant-a", "gui-1", "cmd-a", Some("key-a"))
                .await
                .expect("oldest tenant-a evicted"),
            LedgerCheck::New
        );
        match ledger
            .check("tenant-b", "gui-2", "cmd-b", Some("key-b"))
            .await
            .expect("tenant-b kept")
        {
            LedgerCheck::Replay(json) => assert_eq!(json, "b"),
            other => panic!("expected replay, got {other:?}"),
        }
        match ledger
            .check("tenant-c", "cli", "cmd-c", None)
            .await
            .expect("tenant-c kept")
        {
            LedgerCheck::Replay(json) => assert_eq!(json, "c"),
            other => panic!("expected replay, got {other:?}"),
        }
        store.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn open_read_only_does_not_reclaim_inflight() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ledger.sqlite3");
        let (store, _) = SessionStore::open(&path).await.expect("open");
        assert_eq!(
            store
                .command_ledger()
                .check("t", "s", "cmd-inflight", None)
                .await
                .expect("reserve inflight"),
            LedgerCheck::New
        );
        store
            .shutdown()
            .await
            .expect("close writer before read-only");

        let readonly = SessionStore::open_read_only(&path)
            .await
            .expect("open_read_only");
        let stats = readonly.command_ledger().stats().await.expect("stats");
        assert_eq!(
            stats.inflight, 1,
            "read-only open must not reclaim inflight rows"
        );
        readonly.shutdown().await.expect("close read-only");

        let (store, _) = SessionStore::open(&path).await.expect("reopen reclaim");
        assert_eq!(
            store
                .command_ledger()
                .check("t", "s", "cmd-inflight", None)
                .await
                .expect("reclaimed"),
            LedgerCheck::New
        );
        store.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn stats_returns_error_when_actor_is_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ledger.sqlite3");
        let (store, _) = SessionStore::open(&path).await.expect("open");
        let ledger = store.command_ledger();
        store.shutdown().await.expect("shutdown");
        let err = ledger.stats().await.expect_err("closed actor must fail");
        assert!(matches!(err, LedgerError::Database(_)));
    }
}
