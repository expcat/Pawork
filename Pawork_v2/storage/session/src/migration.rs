use std::path::Path;

use pawork_sqlite::{DatabaseActor, Migration, MigrationReport};

use crate::SessionStoreError;

pub const CURRENT_SCHEMA_VERSION: u32 = 9;
const SCHEMA_MIGRATIONS_TABLE: &str = "schema_migrations";

pub(crate) const MIGRATIONS: &[Migration] = &[
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
    Migration {
        version: 7,
        name: "client_adapter_session_registry",
        sql: r#"
            CREATE TABLE client_adapter_sessions (
                client_session_id TEXT PRIMARY KEY,
                schema_version INTEGER NOT NULL CHECK (schema_version > 0),
                protocol TEXT NOT NULL,
                core_session_id TEXT NOT NULL,
                connection_id TEXT NOT NULL,
                ownership_epoch INTEGER NOT NULL CHECK (ownership_epoch >= 0),
                revision INTEGER NOT NULL CHECK (revision >= 0),
                state TEXT NOT NULL,
                capability_json TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
            );
            CREATE INDEX idx_client_adapter_core_session
                ON client_adapter_sessions(core_session_id);
        "#,
    },
    Migration {
        version: 8,
        name: "session_identity_tenant_backfill",
        // P18-2（ADR-033）：legacy session 补 tenant/principal 列并 backfill 到
        // local/default + local/user。NOT NULL DEFAULT 对既有行生效（SQLite
        // ADD COLUMN 语义），显式 UPDATE 兜底任何非空旧值；迁移在单事务内执行，
        // 失败整批回滚，账本保证重复执行为 no-op。
        sql: r#"
            ALTER TABLE sessions
                ADD COLUMN tenant_id TEXT NOT NULL DEFAULT 'local/default';
            ALTER TABLE sessions
                ADD COLUMN principal_id TEXT NOT NULL DEFAULT 'local/user';
            CREATE INDEX idx_sessions_tenant ON sessions(tenant_id);
            UPDATE sessions
                SET tenant_id = 'local/default', principal_id = 'local/user'
                WHERE tenant_id IS NULL OR tenant_id = ''
                   OR principal_id IS NULL OR principal_id = '';
        "#,
    },
    Migration {
        version: 9,
        name: "session_binding_affinity",
        // P18-7（ADR-033）：session affinity / binding 的持久化投影——flat snapshot
        // 行（tenant+session+agent 复合主键，state 冻结词表 bound/rebinding/released，
        // revision+ownership_epoch 供原子 CAS）与 append-only 事件日志（重放 / 审计）。
        // 不含任何 secret 列：只存 opaque 定位符与 lease 引用。
        sql: r#"
            CREATE TABLE session_bindings (
                tenant_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                schema_version INTEGER NOT NULL CHECK (schema_version > 0),
                state TEXT NOT NULL CHECK (
                    state IN ('bound', 'rebinding', 'released')
                ),
                revision INTEGER NOT NULL CHECK (revision > 0),
                ownership_epoch INTEGER NOT NULL CHECK (ownership_epoch >= 0),
                provider_id TEXT NOT NULL,
                model_id TEXT NOT NULL,
                account_id TEXT NOT NULL,
                credential_id TEXT NOT NULL,
                capability_hash INTEGER NOT NULL,
                policy_hash INTEGER NOT NULL,
                lease_id TEXT NOT NULL,
                bound_at_ms INTEGER NOT NULL CHECK (bound_at_ms >= 0),
                ttl_ms INTEGER NOT NULL CHECK (ttl_ms >= 0),
                expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms >= 0),
                PRIMARY KEY (tenant_id, session_id, agent_id)
            );
            CREATE INDEX idx_session_bindings_session
                ON session_bindings(session_id);
            CREATE TABLE session_binding_events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                tenant_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                event_json TEXT NOT NULL,
                appended_at_ms INTEGER NOT NULL CHECK (appended_at_ms >= 0)
            );
            CREATE INDEX idx_session_binding_events_key
                ON session_binding_events(tenant_id, session_id, agent_id, seq);
        "#,
    },
];

pub(crate) async fn migrate(
    database: &DatabaseActor,
    database_path: &Path,
    existed: bool,
) -> Result<MigrationReport, SessionStoreError> {
    pawork_sqlite::migrate(
        database,
        SCHEMA_MIGRATIONS_TABLE,
        MIGRATIONS,
        CURRENT_SCHEMA_VERSION,
        database_path,
        existed,
    )
    .await
    .map_err(SessionStoreError::from)
}

pub(crate) async fn schema_version(database: &DatabaseActor) -> Result<u32, SessionStoreError> {
    pawork_sqlite::schema_version(database, SCHEMA_MIGRATIONS_TABLE)
        .await
        .map_err(SessionStoreError::from)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pawork_sqlite::{DatabaseActor, Migration, MigrationError};

    use super::*;
    use crate::SessionStore;

    fn temp_db(name: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(name);
        (dir, path)
    }

    fn seed_v6_ledger_and_sessions(connection: &mut rusqlite::Connection) -> rusqlite::Result<()> {
        connection.execute_batch(
            "CREATE TABLE schema_migrations (\
             version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at_ms INTEGER NOT NULL\
             ); \
             INSERT INTO schema_migrations VALUES (1,'core_session_schema',0); \
             INSERT INTO schema_migrations VALUES (2,'event_store_immutability',0); \
             INSERT INTO schema_migrations VALUES (3,'branch_active_and_session_leases',0); \
             INSERT INTO schema_migrations VALUES (4,'session_tags_and_search',0); \
             INSERT INTO schema_migrations VALUES (5,'server_tool_events_and_transcript_envelopes',0); \
             INSERT INTO schema_migrations VALUES (6,'compat_import_identity',0); \
             CREATE TABLE sessions (\
                 session_id TEXT PRIMARY KEY,\
                 title TEXT NOT NULL DEFAULT '',\
                 created_at_ms INTEGER NOT NULL,\
                 updated_at_ms INTEGER NOT NULL,\
                 archived INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1)),\
                 active_branch TEXT NOT NULL DEFAULT 'main'\
             ); \
             INSERT INTO sessions(session_id, title, created_at_ms, updated_at_ms) \
             VALUES ('legacy-session-1', 'old', 1, 1);",
        )
    }

    #[tokio::test]
    async fn new_database_migrates_to_current_schema() {
        let (_dir, path) = temp_db("new.sqlite3");
        let (store, report) = SessionStore::open(&path).await.expect("open store");
        assert_eq!(report.from_version, 0);
        assert_eq!(report.to_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(report.applied_versions, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);
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
            "client_adapter_sessions",
            "messages",
            "runs",
            "server_tool_events",
            "session_binding_events",
            "session_bindings",
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
    }

    #[tokio::test]
    async fn existing_database_is_backed_up_before_forward_migration() {
        let (_dir, path) = temp_db("legacy.sqlite3");
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
    }

    #[tokio::test]
    async fn legacy_sessions_backfill_to_local_default_identity() {
        // 模拟 v6 时代的 legacy 库：sessions 表无 tenant/principal 列，已有旧行。
        let (_dir, path) = temp_db("legacy-v6.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("actor");
        actor
            .call(seed_v6_ledger_and_sessions)
            .await
            .expect("actor")
            .expect("seed legacy v6");
        actor.shutdown().await.expect("shutdown");

        let (store, report) = SessionStore::open(&path).await.expect("migrate");
        assert_eq!(report.from_version, 6);
        assert_eq!(report.to_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(report.applied_versions, vec![7, 8, 9]);
        let (tenant, principal): (String, String) = store
            .database()
            .call(|connection| {
                connection.query_row(
                    "SELECT tenant_id, principal_id FROM sessions WHERE session_id='legacy-session-1'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .await
            .expect("actor")
            .expect("backfilled row");
        assert_eq!(tenant, "local/default", "legacy session 必须回填默认租户");
        assert_eq!(principal, "local/user", "legacy session 必须回填默认主体");

        // 幂等：再次打开不重放迁移，行值不变。
        store.shutdown().await.expect("shutdown");
        let (store, second) = SessionStore::open(&path).await.expect("re-migrate");
        assert!(second.applied_versions.is_empty());
        let tenant: String = store
            .database()
            .call(|connection| {
                connection.query_row(
                    "SELECT tenant_id FROM sessions WHERE session_id='legacy-session-1'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("actor")
            .expect("row");
        assert_eq!(tenant, "local/default");
        store.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn failing_v8_rolls_back_tenant_columns() {
        // 对 v6 种子库调用通用 runner：完整 1–9 计划，但 v8 SQL 含语法错误。
        // 整批事务回滚后不得残留 tenant 列，账本仍为 6。
        let (_dir, path) = temp_db("failure-v8.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("actor");
        actor
            .call(seed_v6_ledger_and_sessions)
            .await
            .expect("actor")
            .expect("seed");
        actor.shutdown().await.expect("shutdown");

        let actor = DatabaseActor::open(&path).await.expect("reopen");
        let mut plan = MIGRATIONS.to_vec();
        plan[7] = Migration {
            version: 8,
            name: MIGRATIONS[7].name,
            sql: "ALTER TABLE sessions ADD COLUMN tenant_id TEXT NOT NULL DEFAULT 'local/default'; \
                  CREATE TABL invalid syntax",
        };
        let error = pawork_sqlite::migrate(
            &actor,
            SCHEMA_MIGRATIONS_TABLE,
            &plan,
            CURRENT_SCHEMA_VERSION,
            &path,
            true,
        )
        .await
        .expect_err("v8 应失败");
        assert!(matches!(
            error,
            MigrationError::MigrationFailed { version: 8, .. }
        ));
        let columns: Vec<String> = actor
            .call(|connection| {
                let mut statement = connection
                    .prepare("PRAGMA table_info(sessions)")
                    .expect("prepare");
                statement
                    .query_map([], |row| row.get::<_, String>(1))
                    .expect("query")
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .expect("collect")
            })
            .await
            .expect("actor");
        assert!(
            !columns.iter().any(|column| column == "tenant_id"),
            "失败迁移不得残留 tenant_id 列"
        );
        let ledger = pawork_sqlite::schema_version(&actor, SCHEMA_MIGRATIONS_TABLE)
            .await
            .expect("ledger");
        assert_eq!(ledger, 6, "账本必须仍为 v6");
        actor.shutdown().await.expect("shutdown");
    }
}
