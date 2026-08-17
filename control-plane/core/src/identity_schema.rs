//! 身份注册表持久化迁移与 legacy backfill（P18-2，ADR-033）。
//!
//! 从 V1 `app-database/identity.rs` 收回。创建版本化、tenant-bound 的
//! `identity_tenants` 注册表，并把 legacy 本地单用户身份
//! （`tenant_id = local/default`、`principal_id = local/user`）幂等种入
//! （INSERT OR IGNORE）。迁移经 [`pawork_sqlite`] 运行器在单事务内执行：
//! 任一迁移失败整批回滚，已存在的库先备份（可整库恢复），账本表保证
//! 重复执行为 no-op（幂等）。SQL 与版本号保持 V1
//! （`CURRENT_IDENTITY_SCHEMA_VERSION = 2`，表名 `identity_schema_migrations`）。

use std::path::Path;

use rusqlite::OptionalExtension;

use pawork_sqlite::{migrate as run_migrations, schema_version as read_schema_version};
use pawork_sqlite::{DatabaseActor, Migration, MigrationError, MigrationReport};

/// 身份注册表 schema 当前版本。
pub const CURRENT_IDENTITY_SCHEMA_VERSION: u32 = 2;

/// 身份迁移账本表名（独立命名空间，与 control-plane / session-store 隔离）。
pub const IDENTITY_MIGRATIONS_TABLE: &str = "identity_schema_migrations";

/// Legacy 默认租户（ADR-033：`local/default`）。
pub const LEGACY_TENANT: &str = "local/default";
/// Legacy 默认主体（ADR-033：`local/user`）。
pub const LEGACY_PRINCIPAL: &str = "local/user";

const IDENTITY_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "identity_tenants_registry_and_default",
        sql: r#"
            CREATE TABLE identity_tenants (
                tenant_id TEXT PRIMARY KEY,
                principal_id TEXT NOT NULL,
                display_name TEXT NOT NULL DEFAULT '',
                schema_version INTEGER NOT NULL CHECK (schema_version > 0),
                created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0)
            );
            INSERT OR IGNORE INTO identity_tenants
                (tenant_id, principal_id, display_name, schema_version, created_at_ms)
            VALUES
                ('local/default', 'local/user', 'Local default identity', 1, 0);
        "#,
    },
    Migration {
        version: 2,
        name: "identity_tenants_composite_key",
        sql: r#"
            ALTER TABLE identity_tenants RENAME TO identity_tenants_v1;
            CREATE TABLE identity_tenants (
                tenant_id TEXT NOT NULL,
                principal_id TEXT NOT NULL,
                display_name TEXT NOT NULL DEFAULT '',
                schema_version INTEGER NOT NULL CHECK (schema_version > 0),
                created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
                PRIMARY KEY (tenant_id, principal_id)
            );
            INSERT INTO identity_tenants
                (tenant_id, principal_id, display_name, schema_version, created_at_ms)
            SELECT tenant_id, principal_id, display_name, 2, created_at_ms
            FROM identity_tenants_v1;
            DROP TABLE identity_tenants_v1;
        "#,
    },
];

/// 注册表中的一条身份记录（存储层视图，字段保持 String 避免引入领域依赖）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityTenant {
    pub tenant_id: String,
    pub principal_id: String,
    pub display_name: String,
    pub schema_version: u32,
    pub created_at_ms: u64,
}

/// 读取身份注册表 schema 版本（未迁移返回 0）。
pub async fn schema_version(database: &DatabaseActor) -> Result<u32, MigrationError> {
    read_schema_version(database, IDENTITY_MIGRATIONS_TABLE).await
}

/// 执行身份注册表前向迁移；已存在的库先备份（rollback 基线）。
pub async fn migrate(
    database: &DatabaseActor,
    database_path: &Path,
    existed: bool,
) -> Result<MigrationReport, MigrationError> {
    run_migrations(
        database,
        IDENTITY_MIGRATIONS_TABLE,
        IDENTITY_MIGRATIONS,
        CURRENT_IDENTITY_SCHEMA_VERSION,
        database_path,
        existed,
    )
    .await
}

/// 幂等 backfill：确保默认本地身份在注册表中存在（INSERT OR IGNORE）。
///
/// 迁移已在建表时种入默认身份；本函数供运行时兜底（如旧库先于本迁移集
/// 升级、或注册表被外部清理后重建），重复调用为 no-op。
pub async fn backfill_legacy_default_identity(
    database: &DatabaseActor,
) -> Result<(), MigrationError> {
    database
        .call(|connection| -> rusqlite::Result<()> {
            connection.execute(
                "INSERT OR IGNORE INTO identity_tenants \
                 (tenant_id, principal_id, display_name, schema_version, created_at_ms) \
                 VALUES (?1, ?2, 'Local default identity', 2, 0)",
                rusqlite::params![LEGACY_TENANT, LEGACY_PRINCIPAL],
            )?;
            Ok(())
        })
        .await??;
    Ok(())
}

/// 按复合键读取指定租户主体记录（不存在返回 `None`）。
pub async fn identity_tenant(
    database: &DatabaseActor,
    tenant_id: &str,
    principal_id: &str,
) -> Result<Option<IdentityTenant>, MigrationError> {
    let tenant_id = tenant_id.to_string();
    let principal_id = principal_id.to_string();
    let row = database
        .call(
            move |connection| -> rusqlite::Result<Option<IdentityTenant>> {
                let row = connection
                .query_row(
                    "SELECT tenant_id, principal_id, display_name, schema_version, created_at_ms \
                     FROM identity_tenants WHERE tenant_id = ?1 AND principal_id = ?2",
                    rusqlite::params![tenant_id, principal_id],
                    |row| {
                        Ok(IdentityTenant {
                            tenant_id: row.get(0)?,
                            principal_id: row.get(1)?,
                            display_name: row.get(2)?,
                            schema_version: row.get::<_, i64>(3)? as u32,
                            created_at_ms: row.get::<_, i64>(4)? as u64,
                        })
                    },
                )
                .optional()?;
                Ok(row)
            },
        )
        .await??;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_path(name: &str) -> PathBuf {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pawork-identity-migration-{}-{unique}-{name}",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn fresh_database_seeds_default_identity() {
        let path = temp_path("fresh.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        let report = migrate(&actor, &path, false).await.expect("migrate");
        assert_eq!(report.from_version, 0);
        assert_eq!(report.to_version, CURRENT_IDENTITY_SCHEMA_VERSION);
        assert_eq!(report.applied_versions, vec![1, 2]);
        assert!(report.backup_path.is_none());

        let tenant = identity_tenant(&actor, LEGACY_TENANT, LEGACY_PRINCIPAL)
            .await
            .expect("read")
            .expect("default identity must be seeded");
        assert_eq!(tenant.tenant_id, "local/default");
        assert_eq!(tenant.principal_id, "local/user");
        assert_eq!(tenant.schema_version, 2);
        assert_eq!(
            identity_tenant(&actor, "tenant-b", "principal-b")
                .await
                .unwrap(),
            None
        );
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn re_migrate_is_idempotent_noop() {
        let path = temp_path("idempotent.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        migrate(&actor, &path, false).await.expect("first migrate");
        let second = migrate(&actor, &path, true).await.expect("re-migrate");
        assert!(second.applied_versions.is_empty());
        assert_eq!(second.from_version, 2);
        assert_eq!(second.to_version, 2);
        // 默认身份只存在一条。
        let count: i64 = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM identity_tenants WHERE tenant_id = 'local/default'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("actor")
            .expect("count");
        assert_eq!(count, 1);
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn runtime_backfill_is_idempotent() {
        let path = temp_path("backfill.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        migrate(&actor, &path, false).await.expect("migrate");
        backfill_legacy_default_identity(&actor)
            .await
            .expect("backfill");
        backfill_legacy_default_identity(&actor)
            .await
            .expect("backfill again");
        let count: i64 = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM identity_tenants WHERE tenant_id = 'local/default'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("actor")
            .expect("count");
        assert_eq!(count, 1, "重复 backfill 不得产生重复记录");
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn existing_database_is_backed_up_and_rolls_back() {
        let path = temp_path("existing.sqlite3");
        let legacy = DatabaseActor::open(&path).await.expect("legacy open");
        legacy
            .call(|connection| {
                connection.execute_batch(
                    "CREATE TABLE legacy_marker(value TEXT); \
                     INSERT INTO legacy_marker VALUES ('pre-migration');",
                )
            })
            .await
            .expect("actor")
            .expect("seed legacy");
        legacy.shutdown().await.expect("legacy shutdown");

        let actor = DatabaseActor::open(&path).await.expect("reopen");
        let report = migrate(&actor, &path, true).await.expect("migrate");
        let backup = report.backup_path.clone().expect("backup path");
        assert!(backup.exists());
        assert!(identity_tenant(&actor, LEGACY_TENANT, LEGACY_PRINCIPAL)
            .await
            .unwrap()
            .is_some());

        // 备份回滚：恢复后 identity 表消失，legacy 数据保留（失败迁移的恢复基线）。
        actor.restore_from(&backup).await.expect("restore");
        let tables: Vec<String> = actor
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
        assert!(!tables.iter().any(|name| name == "identity_tenants"));
        assert!(
            tables.iter().any(|name| name == "legacy_marker"),
            "legacy 数据应保留"
        );
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(backup);
    }

    #[tokio::test]
    async fn failed_migration_rolls_back_whole_batch() {
        let path = temp_path("failure.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        let bad = Migration {
            version: 1,
            name: "bad",
            sql: "CREATE TABL invalid syntax",
        };
        let error = pawork_sqlite::migrate(&actor, IDENTITY_MIGRATIONS_TABLE, &[bad], 1, &path, false)
            .await
            .expect_err("应迁移失败");
        assert!(matches!(
            error,
            MigrationError::MigrationFailed { version: 1, .. }
        ));
        // 账本表仍创建，但无成功版本记录；业务表不得残留。
        assert_eq!(schema_version(&actor).await.unwrap(), 0);
        let identity_tables: i64 = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM sqlite_master \
                     WHERE type='table' AND name='identity_tenants'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("actor")
            .expect("query");
        assert_eq!(identity_tables, 0, "失败迁移不得留下半迁移状态");
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn one_tenant_can_register_multiple_principals() {
        let path = temp_path("multi-principal.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        migrate(&actor, &path, false).await.expect("migrate");
        actor
            .call(|connection| {
                connection.execute(
                    "INSERT INTO identity_tenants \
                     (tenant_id, principal_id, display_name, schema_version, created_at_ms) \
                     VALUES ('local/default', 'automation:worker', 'Worker', 2, 1)",
                    [],
                )
            })
            .await
            .expect("actor")
            .expect("insert second principal");

        let local = identity_tenant(&actor, LEGACY_TENANT, LEGACY_PRINCIPAL)
            .await
            .expect("read local")
            .expect("local user");
        let worker = identity_tenant(&actor, LEGACY_TENANT, "automation:worker")
            .await
            .expect("read worker")
            .expect("worker");
        assert_eq!(local.principal_id, LEGACY_PRINCIPAL);
        assert_eq!(worker.principal_id, "automation:worker");
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn v1_to_v2_upgrades_to_composite_primary_key() {
        let path = temp_path("v1-to-v2.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        let v1_report = pawork_sqlite::migrate(
            &actor,
            IDENTITY_MIGRATIONS_TABLE,
            &IDENTITY_MIGRATIONS[..1],
            1,
            &path,
            false,
        )
        .await
        .expect("apply v1");
        assert_eq!(v1_report.applied_versions, vec![1]);
        assert_eq!(schema_version(&actor).await.unwrap(), 1);

        let v1_sql: String = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT sql FROM sqlite_master WHERE type='table' AND name='identity_tenants'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("actor")
            .expect("v1 sql");
        assert!(
            v1_sql.contains("tenant_id TEXT PRIMARY KEY"),
            "v1 必须是 tenant_id 单列主键：{v1_sql}"
        );

        let second_principal = actor
            .call(|connection| {
                connection.execute(
                    "INSERT INTO identity_tenants \
                     (tenant_id, principal_id, display_name, schema_version, created_at_ms) \
                     VALUES ('local/default', 'automation:worker', 'Worker', 1, 1)",
                    [],
                )
            })
            .await
            .expect("actor");
        assert!(
            second_principal.is_err(),
            "v1 单列主键不得登记同一租户的第二个主体"
        );

        let seeded = identity_tenant(&actor, LEGACY_TENANT, LEGACY_PRINCIPAL)
            .await
            .expect("read")
            .expect("legacy seed");
        assert_eq!(seeded.tenant_id, "local/default");
        assert_eq!(seeded.principal_id, "local/user");
        assert_eq!(seeded.schema_version, 1);

        let v2_report = migrate(&actor, &path, true).await.expect("apply v2");
        assert_eq!(v2_report.applied_versions, vec![2]);
        assert_eq!(
            schema_version(&actor).await.unwrap(),
            CURRENT_IDENTITY_SCHEMA_VERSION
        );

        let v2_sql: String = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT sql FROM sqlite_master WHERE type='table' AND name='identity_tenants'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("actor")
            .expect("v2 sql");
        assert!(
            v2_sql.contains("PRIMARY KEY (tenant_id, principal_id)"),
            "v2 必须升级为复合主键：{v2_sql}"
        );

        actor
            .call(|connection| {
                connection.execute(
                    "INSERT INTO identity_tenants \
                     (tenant_id, principal_id, display_name, schema_version, created_at_ms) \
                     VALUES ('local/default', 'automation:worker', 'Worker', 2, 1)",
                    [],
                )
            })
            .await
            .expect("actor")
            .expect("insert second principal after v2");

        let upgraded = identity_tenant(&actor, LEGACY_TENANT, LEGACY_PRINCIPAL)
            .await
            .expect("read upgraded")
            .expect("legacy seed preserved");
        assert_eq!(upgraded.schema_version, 2);
        assert_eq!(upgraded.principal_id, LEGACY_PRINCIPAL);
        let worker = identity_tenant(&actor, LEGACY_TENANT, "automation:worker")
            .await
            .expect("read worker")
            .expect("worker");
        assert_eq!(worker.principal_id, "automation:worker");

        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
        if let Some(backup) = v2_report.backup_path {
            let _ = fs::remove_file(backup);
        }
    }
}
