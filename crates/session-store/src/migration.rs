use std::path::{Path, PathBuf};

use app_database::DatabaseActor;
use rusqlite::{params, OptionalExtension};

use crate::SessionStoreError;

pub const CURRENT_SCHEMA_VERSION: u32 = 6;

struct Migration {
    version: u32,
    name: &'static str,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "core_session_schema",
        sql: r#"
        CREATE TABLE sessions (
            session_id TEXT PRIMARY KEY,
            title TEXT NOT NULL DEFAULT '',
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            archived INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1))
        );
        CREATE TABLE session_branches (
            session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
            branch_id TEXT NOT NULL,
            parent_branch_id TEXT,
            forked_from_event_id TEXT,
            head_sequence INTEGER NOT NULL DEFAULT 0 CHECK (head_sequence >= 0),
            PRIMARY KEY(session_id, branch_id)
        );
        CREATE TABLE session_events (
            event_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE RESTRICT,
            branch_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            parent_event_id TEXT REFERENCES session_events(event_id),
            sequence INTEGER NOT NULL CHECK (sequence > 0),
            event_type TEXT NOT NULL,
            schema_version INTEGER NOT NULL CHECK (schema_version > 0),
            timestamp_ms INTEGER NOT NULL CHECK (timestamp_ms >= 0),
            payload_json TEXT NOT NULL,
            UNIQUE(session_id, sequence),
            FOREIGN KEY(session_id, branch_id) REFERENCES session_branches(session_id, branch_id) ON DELETE RESTRICT
        );
        CREATE INDEX idx_session_events_replay ON session_events(session_id, sequence);
        CREATE TABLE messages (
            message_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
            run_id TEXT NOT NULL,
            sequence INTEGER NOT NULL,
            role TEXT NOT NULL,
            message_json TEXT NOT NULL
        );
        CREATE INDEX idx_messages_session_sequence ON messages(session_id, sequence);
        CREATE TABLE runs (
            run_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
            state TEXT NOT NULL,
            started_at_ms INTEGER,
            completed_at_ms INTEGER,
            run_json TEXT NOT NULL DEFAULT '{}'
        );
        CREATE INDEX idx_runs_session ON runs(session_id);
        CREATE TABLE tool_calls (
            tool_call_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
            run_id TEXT NOT NULL,
            name TEXT NOT NULL,
            state TEXT NOT NULL,
            arguments_json TEXT NOT NULL DEFAULT '',
            result_json TEXT
        );
        CREATE INDEX idx_tool_calls_run ON tool_calls(run_id);
    "#,
    },
    Migration {
        version: 2,
        name: "event_store_immutability",
        sql: r#"
            CREATE TRIGGER session_events_no_update
            BEFORE UPDATE ON session_events
            BEGIN
                SELECT RAISE(ABORT, 'session_events is append-only');
            END;
            CREATE TRIGGER session_events_no_delete
            BEFORE DELETE ON session_events
            BEGIN
                SELECT RAISE(ABORT, 'session_events is append-only');
            END;
        "#,
    },
    Migration {
        version: 3,
        name: "branch_active_and_session_leases",
        sql: r#"
            ALTER TABLE sessions
                ADD COLUMN active_branch TEXT NOT NULL DEFAULT 'main';
            CREATE INDEX idx_session_events_branch_sequence
                ON session_events(session_id, branch_id, sequence);
            CREATE TABLE session_leases (
                session_id TEXT PRIMARY KEY REFERENCES sessions(session_id) ON DELETE CASCADE,
                holder TEXT NOT NULL,
                acquired_at_ms INTEGER NOT NULL CHECK (acquired_at_ms >= 0),
                expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms >= 0),
                CHECK (acquired_at_ms <= expires_at_ms)
            );
        "#,
    },
    Migration {
        version: 4,
        name: "session_tags_and_search",
        sql: r#"
            CREATE TABLE IF NOT EXISTS session_tags (
                session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
                tag TEXT NOT NULL,
                PRIMARY KEY(session_id, tag)
            );
            CREATE INDEX IF NOT EXISTS idx_session_tags_tag ON session_tags(tag);
        "#,
    },
    Migration {
        version: 5,
        name: "server_tool_events_and_transcript_envelopes",
        sql: r#"
            CREATE TABLE server_tool_events (
                tool_call_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
                run_id TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                name TEXT NOT NULL,
                state TEXT NOT NULL,
                arguments_json TEXT NOT NULL DEFAULT '',
                command TEXT,
                citations_json TEXT NOT NULL DEFAULT '[]',
                sources_json TEXT NOT NULL DEFAULT '[]',
                screenshots_json TEXT NOT NULL DEFAULT '[]',
                outputs_json TEXT NOT NULL DEFAULT '[]',
                result_json TEXT,
                error_json TEXT
            );
            CREATE INDEX idx_server_tool_events_session
                ON server_tool_events(session_id);
            CREATE TABLE transcript_envelopes (
                session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
                run_id TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                envelope_json TEXT NOT NULL,
                PRIMARY KEY(session_id, sequence)
            );
        "#,
    },
    Migration {
        version: 6,
        name: "compat_import_identity",
        sql: r#"
            CREATE TABLE compat_import_identity (
                source TEXT NOT NULL,
                original_id TEXT NOT NULL,
                content_fingerprint TEXT NOT NULL,
                session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
                PRIMARY KEY (source, original_id)
            );
            CREATE INDEX idx_compat_import_identity_session
                ON compat_import_identity(session_id);
        "#,
    },
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationReport {
    pub from_version: u32,
    pub to_version: u32,
    pub applied_versions: Vec<u32>,
    pub backup_path: Option<PathBuf>,
}

pub(crate) async fn migrate(
    database: &DatabaseActor,
    database_path: &Path,
    existed: bool,
) -> Result<MigrationReport, SessionStoreError> {
    let from_version = schema_version(database).await?;
    if from_version > CURRENT_SCHEMA_VERSION {
        return Err(SessionStoreError::UnsupportedSchema {
            found: from_version,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }
    if from_version == CURRENT_SCHEMA_VERSION {
        return Ok(MigrationReport {
            from_version,
            to_version: from_version,
            applied_versions: Vec::new(),
            backup_path: None,
        });
    }

    let backup_path = if existed {
        let path = backup_path(database_path, from_version);
        database.backup_to(&path).await?;
        Some(path)
    } else {
        None
    };

    let pending: Vec<(u32, &'static str, &'static str)> = MIGRATIONS
        .iter()
        .filter(|migration| migration.version > from_version)
        .map(|migration| (migration.version, migration.name, migration.sql))
        .collect();
    let applied = database
        .call(move |connection| -> Result<Vec<u32>, SessionStoreError> {
            connection.execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migrations (\
                 version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at_ms INTEGER NOT NULL\
                 );",
            )?;
            let transaction = connection.transaction()?;
            let mut applied = Vec::new();
            for (version, name, sql) in pending {
                if let Err(error) = transaction.execute_batch(sql) {
                    return Err(SessionStoreError::MigrationFailed {
                        version,
                        name,
                        message: error.to_string(),
                    });
                }
                transaction.execute(
                    "INSERT INTO schema_migrations(version, name, applied_at_ms) \
                     VALUES (?1, ?2, CAST(strftime('%s','now') AS INTEGER) * 1000)",
                    params![version, name],
                )?;
                transaction.pragma_update(None, "user_version", version)?;
                applied.push(version);
            }
            transaction.commit()?;
            Ok(applied)
        })
        .await??;

    Ok(MigrationReport {
        from_version,
        to_version: CURRENT_SCHEMA_VERSION,
        applied_versions: applied,
        backup_path,
    })
}

pub(crate) async fn schema_version(database: &DatabaseActor) -> Result<u32, SessionStoreError> {
    let version = database
        .call(|connection| -> rusqlite::Result<i64> {
            let has_migrations: Option<i64> = connection
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations'",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            if has_migrations.is_none() {
                return Ok(0);
            }
            connection.query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |row| row.get(0),
            )
        })
        .await??;
    u32::try_from(version).map_err(|_| SessionStoreError::InvalidSchemaVersion(version))
}

fn backup_path(database_path: &Path, from_version: u32) -> PathBuf {
    let file_name = database_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("pawork.sqlite3");
    database_path.with_file_name(format!("{file_name}.pre-migration-v{from_version}.bak"))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use app_database::DatabaseActor;

    use super::*;
    use crate::SessionStore;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_path(name: &str) -> PathBuf {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pawork-session-store-{}-{unique}-{name}",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn new_database_migrates_to_current_schema() {
        let path = temp_path("new.sqlite3");
        let (store, report) = SessionStore::open(&path).await.expect("open store");
        assert_eq!(report.from_version, 0);
        assert_eq!(report.to_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(report.applied_versions, vec![1, 2, 3, 4, 5, 6]);
        assert!(report.backup_path.is_none());
        let tables: Vec<String> = store
            .database()
            .call(|connection| {
                let mut statement = connection
                    .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                    .expect("prepare");
                statement
                    .query_map([], |row| row.get(0))
                    .expect("query")
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .expect("collect")
            })
            .await
            .expect("actor");
        for expected in [
            "messages",
            "runs",
            "server_tool_events",
            "session_events",
            "sessions",
            "tool_calls",
            "transcript_envelopes",
        ] {
            assert!(
                tables.iter().any(|table| table == expected),
                "missing {expected}"
            );
        }
        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn existing_database_is_backed_up_before_forward_migration() {
        let path = temp_path("legacy.sqlite3");
        let legacy = DatabaseActor::open(&path).await.expect("legacy actor");
        legacy
            .call(|connection| {
                connection.execute_batch(
                    "CREATE TABLE legacy(value TEXT); INSERT INTO legacy VALUES ('kept');",
                )
            })
            .await
            .expect("actor")
            .expect("legacy schema");
        legacy.shutdown().await.expect("shutdown");

        let (store, report) = SessionStore::open(&path).await.expect("migrate");
        let backup = report.backup_path.clone().expect("backup path");
        assert!(backup.exists());
        let backup_actor = DatabaseActor::open_read_only(&backup)
            .await
            .expect("backup actor");
        let value: String = backup_actor
            .call(|connection| {
                connection.query_row("SELECT value FROM legacy", [], |row| row.get(0))
            })
            .await
            .expect("actor")
            .expect("legacy value");
        assert_eq!(value, "kept");
        backup_actor.shutdown().await.expect("shutdown");
        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(backup);
    }

    #[tokio::test]
    async fn failed_migration_rolls_back_the_whole_transaction() {
        let path = temp_path("failure.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("actor");
        let result = actor
            .call(|connection| -> rusqlite::Result<()> {
                let transaction = connection.transaction()?;
                transaction.execute_batch("CREATE TABLE should_rollback(id INTEGER);")?;
                let failure = transaction.execute_batch("CREATE TABL invalid syntax");
                assert!(failure.is_err());
                drop(transaction);
                Ok(())
            })
            .await
            .expect("actor");
        result.expect("test transaction");
        let exists: i64 = actor.call(|connection| connection.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='should_rollback'", [], |row| row.get(0))).await.expect("actor").expect("query");
        assert_eq!(exists, 0);
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }
}
