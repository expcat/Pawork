use std::path::Path;

use crate::sqlite::{DatabaseActor, Migration, MigrationReport};

use crate::session::SessionStoreError;

pub const CURRENT_SCHEMA_VERSION: u32 = 14;
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
        // P18-7（ADR-033）曾建 session_bindings/session_binding_events；R0/ADR-038 D3：
        // binding 状态机已归档（tag v2-final），本表无读写方；append-only 留表「预留」，
        // 不回滚 DDL；复活条件见 ROADMAP §3.3/§4。
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
    Migration {
        version: 10,
        name: "messages_branch_projection",
        // F09：messages 是可重建投影，附加 branch_id 供消费面按祖先链过滤。
        // 不改 session_events 信封、append-only 触发器或 UNIQUE(session_id, sequence)。
        sql: r#"
            ALTER TABLE messages
                ADD COLUMN branch_id TEXT NOT NULL DEFAULT 'main';
            UPDATE messages
                SET branch_id = COALESCE(
                    (
                        SELECT e.branch_id
                        FROM session_events e
                        WHERE e.session_id = messages.session_id
                          AND e.sequence = messages.sequence
                    ),
                    'main'
                );
            CREATE INDEX idx_messages_session_branch_sequence
                ON messages(session_id, branch_id, sequence);
        "#,
    },
    Migration {
        version: 11,
        name: "command_ledger",
        sql: r#"
            CREATE TABLE command_ledger (
                tenant_id TEXT NOT NULL,
                client_scope TEXT NOT NULL,
                command_id TEXT NOT NULL,
                idempotency_key TEXT,
                status TEXT NOT NULL CHECK(status IN ('inflight','completed')),
                response_json TEXT,
                created_at_ms INTEGER NOT NULL,
                completed_at_ms INTEGER,
                PRIMARY KEY(tenant_id, client_scope, command_id)
            );
            CREATE UNIQUE INDEX idx_command_ledger_idempotency_key
                ON command_ledger(tenant_id, client_scope, idempotency_key)
                WHERE idempotency_key IS NOT NULL;
        "#,
    },
    Migration {
        version: 12,
        name: "messages_branch_projection_rebuild",
        // R6 波 A（ADR-040 D3/D4）：messages 是可重建投影，branch_id 不得再
        // 依赖 DEFAULT 'main' 静默兜底。回填即校验：缺失事件背书的投影行在
        // 此 fail-closed（单条迁移事务整批回滚），随后按事件所属 branch 重建
        // 整表并恢复两个索引。v1–v10 DDL 与 v11 command_ledger 不改写，只追加。
        sql: r#"
            CREATE TEMP TABLE v12_orphan_check(x TEXT);
            CREATE TEMP TRIGGER v12_orphan_fail BEFORE INSERT ON v12_orphan_check
            BEGIN
                SELECT RAISE(ABORT, 'v12: messages projection row lacks backing session_event');
            END;
            INSERT INTO v12_orphan_check(x)
                SELECT NULL FROM messages m
                WHERE NOT EXISTS (
                    SELECT 1 FROM session_events e
                    WHERE e.session_id = m.session_id
                      AND e.sequence = m.sequence
                );
            DROP TRIGGER v12_orphan_fail;
            DROP TABLE v12_orphan_check;

            CREATE TABLE messages_v12(
                message_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
                run_id TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                role TEXT NOT NULL,
                message_json TEXT NOT NULL,
                branch_id TEXT NOT NULL
            );
            INSERT INTO messages_v12(
                message_id, session_id, run_id, sequence, role, message_json, branch_id
            )
            SELECT m.message_id, m.session_id, m.run_id, m.sequence, m.role, m.message_json,
                   (SELECT e.branch_id FROM session_events e
                    WHERE e.session_id = m.session_id
                      AND e.sequence = m.sequence)
            FROM messages m;
            DROP TABLE messages;
            ALTER TABLE messages_v12 RENAME TO messages;
            CREATE INDEX idx_messages_session_sequence
                ON messages(session_id, sequence);
            CREATE INDEX idx_messages_session_branch_sequence
                ON messages(session_id, branch_id, sequence);
        "#,
    },
    Migration {
        version: 13,
        name: "session_workspace_binding",
        // ADR-043（Accepted 2026-08-31）：Session→Workspace 归属跨 Host 重启
        // 持久化。纯追加可空列，不回填：历史 NULL 继续诚实落入 Unassigned。
        // 不加 FK——workspace 登记是 Host 进程内/按实例恢复的状态，跨 Host
        // 不保证存在；归属列为弱引用，尚未登记的 canonical id 原样保留。
        sql: "ALTER TABLE sessions ADD COLUMN workspace_id TEXT;",
    },
    Migration {
        version: 14,
        name: "persistent_workspace_registry",
        // ADR-044：Host 本地项目注册表。root_path 在写入前由 workspace
        // 层 canonicalize；sessions.workspace_id 继续是无 FK 的弱引用。
        sql: r#"
            CREATE TABLE workspaces (
                workspace_id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                root_path TEXT NOT NULL UNIQUE,
                created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
                updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
            );
            CREATE INDEX idx_workspaces_created
                ON workspaces(created_at_ms, workspace_id);
        "#,
    },
];

pub(crate) async fn migrate(
    database: &DatabaseActor,
    database_path: &Path,
    existed: bool,
) -> Result<MigrationReport, SessionStoreError> {
    crate::sqlite::migrate(
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
    crate::sqlite::schema_version(database, SCHEMA_MIGRATIONS_TABLE)
        .await
        .map_err(SessionStoreError::from)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::sqlite::{DatabaseActor, Migration, MigrationError};

    use super::*;
    use crate::session::test_support as seed;
    use crate::session::SessionStore;
    use pawork_domain::{AgentEventEnvelope, SessionId, Timestamp, WorkspaceId};

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
             VALUES ('legacy-session-1', 'old', 1, 1); \
             CREATE TABLE messages (\
                 message_id TEXT PRIMARY KEY,\
                 session_id TEXT NOT NULL,\
                 run_id TEXT NOT NULL,\
                 sequence INTEGER NOT NULL,\
                 role TEXT NOT NULL,\
                 message_json TEXT NOT NULL\
             ); \
             CREATE TABLE session_events (\
                 event_id TEXT PRIMARY KEY,\
                 session_id TEXT NOT NULL,\
                 branch_id TEXT NOT NULL,\
                 run_id TEXT NOT NULL,\
                 parent_event_id TEXT,\
                 sequence INTEGER NOT NULL,\
                 event_type TEXT NOT NULL,\
                 schema_version INTEGER NOT NULL,\
                 timestamp_ms INTEGER NOT NULL,\
                 payload_json TEXT NOT NULL\
             );",
        )
    }

    #[tokio::test]
    async fn new_database_migrates_to_current_schema() {
        let (_dir, path) = temp_db("new.sqlite3");
        let (store, report) = SessionStore::open(&path).await.expect("open store");
        assert_eq!(report.from_version, 0);
        assert_eq!(report.to_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(
            report.applied_versions,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
        );
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
            "command_ledger",
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
        assert_eq!(report.applied_versions, vec![7, 8, 9, 10, 11, 12, 13, 14]);
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
        // 对 v6 种子库调用通用 runner：完整 1–11 计划，但 v8 SQL 含语法错误。
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
            sql:
                "ALTER TABLE sessions ADD COLUMN tenant_id TEXT NOT NULL DEFAULT 'local/default'; \
                  CREATE TABL invalid syntax",
        };
        let error = crate::sqlite::migrate(
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
        let ledger = crate::sqlite::schema_version(&actor, SCHEMA_MIGRATIONS_TABLE)
            .await
            .expect("ledger");
        assert_eq!(ledger, 6, "账本必须仍为 v6");
        actor.shutdown().await.expect("shutdown");
    }

    fn seed_v9_messages_without_branch_column(
        connection: &mut rusqlite::Connection,
    ) -> rusqlite::Result<()> {
        connection.execute_batch(
            "INSERT INTO sessions(\
                 session_id, title, created_at_ms, updated_at_ms, \
                 active_branch, tenant_id, principal_id\
             ) VALUES ('legacy-v9', 'v9', 1, 1, 'main', 'local/default', 'local/user'); \
             INSERT INTO session_branches(session_id, branch_id, head_sequence) \
             VALUES ('legacy-v9', 'main', 2); \
             INSERT INTO session_branches(\
                 session_id, branch_id, parent_branch_id, forked_from_event_id, head_sequence\
             ) VALUES ('legacy-v9', 'experiment', 'main', 'event-1', 1); \
             INSERT INTO session_events(\
                 event_id, session_id, branch_id, run_id, parent_event_id, sequence, \
                 event_type, schema_version, timestamp_ms, payload_json\
             ) VALUES \
             ('event-1', 'legacy-v9', 'main', 'run-1', NULL, 1, 'message_committed', 1, 1, '{}'), \
             ('event-2', 'legacy-v9', 'experiment', 'run-1', NULL, 2, 'message_committed', 1, 2, '{}'); \
             INSERT INTO messages(\
                 message_id, session_id, run_id, sequence, role, message_json\
             ) VALUES \
             ('m-main', 'legacy-v9', 'run-1', 1, 'user', '{\"id\":\"m-main\"}'), \
             ('m-fork', 'legacy-v9', 'run-1', 2, 'user', '{\"id\":\"m-fork\"}');",
        )
    }

    #[tokio::test]
    async fn v9_database_backfills_message_branch_id() {
        let (_dir, path) = temp_db("legacy-v9-messages.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("actor");
        crate::sqlite::migrate(
            &actor,
            SCHEMA_MIGRATIONS_TABLE,
            &MIGRATIONS[..9],
            9,
            &path,
            false,
        )
        .await
        .expect("apply v1–v9");
        actor
            .call(seed_v9_messages_without_branch_column)
            .await
            .expect("actor")
            .expect("seed v9 messages");
        actor.shutdown().await.expect("shutdown");

        let (store, report) = SessionStore::open(&path).await.expect("migrate to v12");
        assert_eq!(report.from_version, 9);
        assert_eq!(report.to_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(report.applied_versions, vec![10, 11, 12, 13, 14]);

        let rows: Vec<(String, String, i64)> = store
            .database()
            .call(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT message_id, branch_id, sequence FROM messages \
                         WHERE session_id='legacy-v9' ORDER BY sequence",
                    )
                    .expect("prepare");
                statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                    .expect("query")
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .expect("collect")
            })
            .await
            .expect("actor");
        assert_eq!(
            rows,
            vec![
                ("m-main".into(), "main".into(), 1),
                ("m-fork".into(), "experiment".into(), 2),
            ]
        );
        store.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn v10_database_applies_command_ledger_and_unique_constraint() {
        let (_dir, path) = temp_db("legacy-v10-command-ledger.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("actor");
        crate::sqlite::migrate(
            &actor,
            SCHEMA_MIGRATIONS_TABLE,
            &MIGRATIONS[..10],
            10,
            &path,
            false,
        )
        .await
        .expect("apply v1–v10");
        actor.shutdown().await.expect("shutdown");

        let (store, report) = SessionStore::open(&path).await.expect("migrate to v12");
        assert_eq!(report.from_version, 10);
        assert_eq!(report.to_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(report.applied_versions, vec![11, 12, 13, 14]);
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
        assert!(
            tables.iter().any(|table| table == "command_ledger"),
            "missing command_ledger"
        );
        let unique_ok = store
            .database()
            .call(|connection| {
                connection.execute(
                    "INSERT INTO command_ledger(                         tenant_id, client_scope, command_id, idempotency_key, status, created_at_ms                     ) VALUES ('t','s','cmd-1','key-1','completed',1)",
                    [],
                )?;
                let err = connection.execute(
                    "INSERT INTO command_ledger(                         tenant_id, client_scope, command_id, idempotency_key, status, created_at_ms                     ) VALUES ('t','s','cmd-2','key-1','completed',2)",
                    [],
                );
                Ok::<_, rusqlite::Error>(err.is_err())
            })
            .await
            .expect("actor")
            .expect("unique probe");
        assert!(
            unique_ok,
            "idempotency_key unique index must reject duplicates"
        );
        store.shutdown().await.expect("shutdown");
        let (store, second) = SessionStore::open(&path).await.expect("reopen");
        assert!(second.applied_versions.is_empty());
        store.shutdown().await.expect("shutdown");
        let readonly = SessionStore::open_read_only(&path)
            .await
            .expect("open_read_only matches v12");
        readonly.shutdown().await.expect("shutdown");
    }

    async fn build_seed_database(
        name: &str,
        scenario: &seed::SeedScenario,
        schema_version: u32,
    ) -> (tempfile::TempDir, PathBuf) {
        let (dir, path) = temp_db(name);
        let actor = DatabaseActor::open(&path).await.expect("seed actor");
        crate::sqlite::migrate(
            &actor,
            SCHEMA_MIGRATIONS_TABLE,
            &MIGRATIONS[..schema_version as usize],
            schema_version,
            &path,
            false,
        )
        .await
        .expect("apply seed schema");
        seed::seed_scenario(&actor, scenario).await;
        actor.shutdown().await.expect("seed shutdown");
        (dir, path)
    }

    fn render_lineage(events: Vec<AgentEventEnvelope>) -> String {
        let lines: Vec<String> = events
            .iter()
            .map(|envelope| serde_json::to_string(envelope).expect("serialize envelope"))
            .collect();
        format!("{}\n", lines.join("\n"))
    }

    async fn assert_lineage_golden(
        store: &SessionStore,
        session: &SessionId,
        branch: &str,
        fixture: &str,
    ) {
        let events = store
            .events_on_lineage(session, branch, 1, 100)
            .await
            .expect("lineage events");
        assert_eq!(
            render_lineage(events),
            fixture,
            "lineage golden mismatch on {branch}"
        );
    }

    async fn message_rows(
        store: &SessionStore,
        session: &'static str,
    ) -> Vec<(String, String, i64)> {
        store
            .database()
            .call(move |connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT message_id, branch_id, sequence FROM messages  \
                         WHERE session_id=?1 ORDER BY sequence, message_id",
                    )
                    .expect("prepare messages");
                statement
                    .query_map([session], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                    .expect("query messages")
                    .collect::<Result<Vec<_>, _>>()
                    .expect("collect messages")
            })
            .await
            .expect("actor")
    }

    async fn messages_table_ddl(store: &SessionStore) -> String {
        store
            .database()
            .call(|connection| {
                connection
                    .query_row(
                        "SELECT sql FROM sqlite_master WHERE type='table' AND name='messages'",
                        [],
                        |row| row.get(0),
                    )
                    .expect("messages ddl")
            })
            .await
            .expect("actor")
    }

    async fn assert_messages_ddl_has_no_default(store: &SessionStore) {
        let ddl = messages_table_ddl(store).await;
        assert!(
            !ddl.contains("DEFAULT"),
            "v12 重建后的 messages DDL 不得携带 DEFAULT: {ddl}"
        );
    }

    #[tokio::test]
    async fn v10_fork_tree_database_upgrades_to_v12_with_lineage_golden() {
        let scenario = seed::fork_tree_scenario();
        let (_dir, path) = build_seed_database("v10-fork-tree.sqlite3", &scenario, 10).await;

        let (store, report) = SessionStore::open(&path).await.expect("upgrade v10 -> v12");
        assert_eq!(report.from_version, 10);
        assert_eq!(report.to_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(report.applied_versions, vec![11, 12, 13, 14]);
        assert!(report
            .backup_path
            .as_ref()
            .is_some_and(|path| path.exists()));

        let session = SessionId::from(scenario.session);
        assert_lineage_golden(
            &store,
            &session,
            "main",
            include_str!("fixtures/v12_fork_tree.main.jsonl"),
        )
        .await;
        assert_lineage_golden(
            &store,
            &session,
            "fork-a",
            include_str!("fixtures/v12_fork_tree.fork-a.jsonl"),
        )
        .await;
        assert_lineage_golden(
            &store,
            &session,
            "fork-b",
            include_str!("fixtures/v12_fork_tree.fork-b.jsonl"),
        )
        .await;

        let mut expected = Vec::new();
        for sequence in 1..=6i64 {
            expected.push((format!("m-main-{sequence}"), "main".into(), sequence));
        }
        for sequence in 7..=9i64 {
            expected.push((format!("m-fork-a-{sequence}"), "fork-a".into(), sequence));
        }
        for sequence in 10..=11i64 {
            expected.push((format!("m-fork-b-{sequence}"), "fork-b".into(), sequence));
        }
        assert_eq!(message_rows(&store, scenario.session).await, expected);
        assert_messages_ddl_has_no_default(&store).await;

        // 重建一致性：按事件所属 branch 重放投影，行集不变。
        store.rebuild_projection(&session).await.expect("rebuild");
        assert_eq!(message_rows(&store, scenario.session).await, expected);
        store.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn v11_interleaved_database_upgrades_to_v12_with_lineage_golden() {
        let scenario = seed::interleaved_scenario();
        let (_dir, path) = build_seed_database("v11-interleaved.sqlite3", &scenario, 11).await;
        // v11 特有面：command_ledger 行须原样穿过 v12 升级。
        let actor = DatabaseActor::open(&path).await.expect("ledger actor");
        actor
            .call(|connection| {
                connection.execute(
                    "INSERT INTO command_ledger(tenant_id, client_scope, command_id, idempotency_key, status, response_json, created_at_ms, completed_at_ms)  \
                     VALUES ('local/default','r6a-golden','cmd-1','key-1','completed','{\"ok\":true}',7,9)",
                    [],
                )
            })
            .await
            .expect("actor")
            .expect("seed ledger row");
        actor.shutdown().await.expect("ledger shutdown");

        let (store, report) = SessionStore::open(&path).await.expect("upgrade v11 -> v12");
        assert_eq!(report.from_version, 11);
        assert_eq!(report.to_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(report.applied_versions, vec![12, 13, 14]);
        assert!(report
            .backup_path
            .as_ref()
            .is_some_and(|path| path.exists()));

        let session = SessionId::from(scenario.session);
        assert_lineage_golden(
            &store,
            &session,
            "main",
            include_str!("fixtures/v12_interleaved.main.jsonl"),
        )
        .await;
        assert_lineage_golden(
            &store,
            &session,
            "side",
            include_str!("fixtures/v12_interleaved.side.jsonl"),
        )
        .await;

        let ledger: (String, String, String) = store
            .database()
            .call(|connection| {
                connection.query_row(
                    "SELECT command_id, status, COALESCE(response_json, '') FROM command_ledger",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
            })
            .await
            .expect("actor")
            .expect("ledger row");
        assert_eq!(
            ledger,
            ("cmd-1".into(), "completed".into(), "{\"ok\":true}".into())
        );

        let expected = vec![
            ("m-1".into(), "main".into(), 1),
            ("m-side-2".into(), "side".into(), 2),
            ("m-3".into(), "main".into(), 3),
            ("m-side-4".into(), "side".into(), 4),
            ("m-5".into(), "main".into(), 5),
            ("m-side-6".into(), "side".into(), 6),
        ];
        assert_eq!(message_rows(&store, scenario.session).await, expected);
        assert_messages_ddl_has_no_default(&store).await;
        store.rebuild_projection(&session).await.expect("rebuild");
        assert_eq!(message_rows(&store, scenario.session).await, expected);
        store.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn v10_compaction_database_upgrades_to_v12_keeping_frozen_fold_semantics() {
        let scenario = seed::compaction_scenario();
        let (_dir, path) = build_seed_database("v10-compaction.sqlite3", &scenario, 10).await;

        let (store, report) = SessionStore::open(&path).await.expect("upgrade v10 -> v12");
        assert_eq!(report.from_version, 10);
        assert_eq!(report.applied_versions, vec![11, 12, 13, 14]);

        let session = SessionId::from(scenario.session);
        assert_lineage_golden(
            &store,
            &session,
            "main",
            include_str!("fixtures/v12_compaction.main.jsonl"),
        )
        .await;
        assert_lineage_golden(
            &store,
            &session,
            "side",
            include_str!("fixtures/v12_compaction.side.jsonl"),
        )
        .await;

        // 冻结语义：main 上 <=2 的消息投影保持折叠删除；fork 点之后的
        // 祖先消息行（m-summary）对 side lineage 仍可见。
        let expected = vec![
            ("m-summary".into(), "main".into(), 4),
            ("m-3".into(), "main".into(), 5),
            ("m-side-1".into(), "side".into(), 6),
        ];
        assert_eq!(message_rows(&store, scenario.session).await, expected);
        assert_messages_ddl_has_no_default(&store).await;
        store.rebuild_projection(&session).await.expect("rebuild");
        assert_eq!(message_rows(&store, scenario.session).await, expected);
        store.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn v11_orphan_message_row_fails_v12_migration_and_preserves_v11_state() {
        let scenario = seed::fork_tree_scenario();
        let (_dir, path) = build_seed_database("v11-orphan.sqlite3", &scenario, 11).await;
        let actor = DatabaseActor::open(&path).await.expect("orphan actor");
        actor
            .call(|connection| {
                connection.execute(
                    "INSERT INTO messages(message_id, session_id, run_id, sequence, role, message_json, branch_id)  \
                     VALUES ('m-orphan', 'r6a-fork-tree', 'run-r6a', 999, 'user', '{}', 'main')",
                    [],
                )
            })
            .await
            .expect("actor")
            .expect("seed orphan row");
        actor.shutdown().await.expect("orphan shutdown");

        let Err(error) = SessionStore::open(&path).await else {
            panic!("孤儿投影行必须 fail-closed，open 不应成功");
        };
        let SessionStoreError::MigrationFailed {
            version, message, ..
        } = &error
        else {
            panic!("v12 必须以 MigrationFailed 失败: {error:?}");
        };
        assert_eq!(*version, 12);
        assert!(
            message.contains("lacks backing session_event"),
            "unexpected: {message}"
        );

        // 失败后：账本仍 v11、messages 原样、v10 的 DEFAULT 仍在 DDL 上（未重建）。
        let actor = DatabaseActor::open(&path).await.expect("verify actor");
        let ledger_version = crate::sqlite::schema_version(&actor, SCHEMA_MIGRATIONS_TABLE)
            .await
            .expect("ledger version");
        assert_eq!(ledger_version, 11);
        let (row_count, orphan_branch): (i64, String) = actor
            .call(|connection| {
                connection
                    .query_row(
                        "SELECT COUNT(*),  \
                         (SELECT branch_id FROM messages WHERE message_id='m-orphan')  \
                         FROM messages WHERE session_id='r6a-fork-tree'",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .expect("verify rows")
            })
            .await
            .expect("actor");
        assert_eq!(row_count, 12, "种子 11 行 + 孤儿 1 行原样保留");
        assert_eq!(orphan_branch, "main");
        let ddl: String = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT sql FROM sqlite_master WHERE type='table' AND name='messages'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("actor")
            .expect("messages ddl");
        assert!(
            ddl.contains("DEFAULT 'main'"),
            "失败迁移不得触碰 v10 messages DDL: {ddl}"
        );
        actor.shutdown().await.expect("verify shutdown");

        // 旧路径只读：v11 库可被 raw read-only 打开核对（SessionStore 层
        // 的 v12 闸门由 open_read_only 常量比较保证，不在此重复断言）。
        let reader = DatabaseActor::open_read_only(&path)
            .await
            .expect("read-only open");
        let total: i64 = reader
            .call(|connection| {
                connection.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            })
            .await
            .expect("actor")
            .expect("count messages");
        assert_eq!(total, 12);
        reader.shutdown().await.expect("reader shutdown");
    }

    #[tokio::test]
    async fn v12_database_upgrades_to_v14_with_null_workspace_and_binding_roundtrip() {
        let (_dir, path) = temp_db("v12-workspace-binding.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("actor");
        crate::sqlite::migrate(
            &actor,
            SCHEMA_MIGRATIONS_TABLE,
            &MIGRATIONS[..12],
            12,
            &path,
            false,
        )
        .await
        .expect("apply v1\u{2013}v12");
        actor
            .call(|connection| {
                connection.execute_batch(
                    "INSERT INTO sessions(\
                         session_id, title, created_at_ms, updated_at_ms, \
                         active_branch, tenant_id, principal_id\
                     ) VALUES ('legacy-v12', 'old', 1, 1, 'main', 'local/default', 'local/user'); \
                     INSERT INTO session_branches(session_id, branch_id, head_sequence) \
                     VALUES ('legacy-v12', 'main', 0);",
                )
            })
            .await
            .expect("actor")
            .expect("seed v12 session");
        actor.shutdown().await.expect("shutdown");

        let (store, report) = SessionStore::open(&path).await.expect("migrate to v14");
        assert_eq!(report.from_version, 12);
        assert_eq!(report.to_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(report.applied_versions, vec![13, 14]);
        assert!(report
            .backup_path
            .as_ref()
            .is_some_and(|path| path.exists()));

        // v13 生效：workspace_id 列存在。
        let columns: Vec<String> = store
            .database()
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
            columns.iter().any(|column| column == "workspace_id"),
            "v13 迁移必须添加 workspace_id 列: {columns:?}"
        );

        // v14 只建空注册表，不根据 legacy session 归属猜 root。
        assert!(store
            .list_workspaces()
            .await
            .expect("list workspaces")
            .is_empty());

        // 旧会话不回填：历史 NULL 继续落入 Unassigned。
        let record = store
            .get_session(&SessionId::from("legacy-v12"))
            .await
            .expect("get legacy");
        assert_eq!(
            record.workspace_id, None,
            "历史会话 workspace_id 必须为 NULL"
        );

        // 绑定写穿 + 目录读回。
        store
            .set_session_workspace(&SessionId::from("legacy-v12"), &WorkspaceId::from("ws-1"))
            .await
            .expect("bind");
        let record = store
            .get_session(&SessionId::from("legacy-v12"))
            .await
            .expect("get bound");
        assert_eq!(record.workspace_id.as_deref(), Some("ws-1"));
        let listed = store.list_sessions().await.expect("list");
        assert!(listed.iter().any(|record| {
            record.session_id == "legacy-v12" && record.workspace_id.as_deref() == Some("ws-1")
        }));
        let atomic = SessionId::from("atomic-v13");
        store
            .create_session_with_workspace(
                &atomic,
                "atomic",
                Timestamp::from_unix_millis(2),
                &WorkspaceId::from("ws-atomic"),
            )
            .await
            .expect("atomic create with workspace");
        assert_eq!(
            store
                .get_session(&atomic)
                .await
                .expect("get atomic")
                .workspace_id
                .as_deref(),
            Some("ws-atomic")
        );

        // 重启后绑定仍在；且不存在的会话 fail-closed。
        store.shutdown().await.expect("shutdown");
        let (store, _) = SessionStore::open(&path).await.expect("reopen");
        let record = store
            .get_session(&SessionId::from("legacy-v12"))
            .await
            .expect("get after reopen");
        assert_eq!(record.workspace_id.as_deref(), Some("ws-1"));
        let error = store
            .set_session_workspace(&SessionId::from("missing"), &WorkspaceId::from("ws-1"))
            .await
            .expect_err("missing session");
        assert!(matches!(
            error,
            SessionStoreError::SessionNotFound(ref id) if id == "missing"
        ));
        store.shutdown().await.expect("shutdown");
    }

    #[test]
    #[ignore = "set PAWORK_WRITE_STORAGE_GOLDEN=1 to refresh fixtures"]
    fn write_v12_upgrade_golden() {
        assert_eq!(
            std::env::var("PAWORK_WRITE_STORAGE_GOLDEN").ok().as_deref(),
            Some("1"),
            "refusing to overwrite golden without PAWORK_WRITE_STORAGE_GOLDEN=1"
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        runtime.block_on(async {
            let fixture_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/session/fixtures");
            std::fs::create_dir_all(fixture_dir).expect("create fixtures dir");
            for scenario in [
                seed::fork_tree_scenario(),
                seed::interleaved_scenario(),
                seed::compaction_scenario(),
            ] {
                let dir = tempfile::tempdir().expect("tempdir");
                let path = dir.path().join("seed.sqlite3");
                let actor = DatabaseActor::open(&path).await.expect("actor");
                crate::sqlite::migrate(
                    &actor,
                    SCHEMA_MIGRATIONS_TABLE,
                    MIGRATIONS,
                    CURRENT_SCHEMA_VERSION,
                    &path,
                    false,
                )
                .await
                .expect("apply current schema");
                seed::seed_scenario(&actor, &scenario).await;
                for branch in scenario.branches.clone() {
                    let scenario_session = scenario.session;
                    let lines = actor
                        .call(move |connection| {
                            seed::lineage_payload_lines(connection, scenario_session, branch)
                        })
                        .await
                        .expect("actor");
                    std::fs::write(
                        format!("{fixture_dir}/{}.{}.jsonl", scenario.name, branch),
                        format!("{}\n", lines.join("\n")),
                    )
                    .expect("write fixture");
                }
                actor.shutdown().await.expect("shutdown");
            }
        });
    }
}
