//! 通用、命名空间化的 SQLite 迁移运行器（P18-1 契约基线）。
//!
//! 每个 migration set 用独立的账本表名（如 `control_plane_schema_migrations`）
//! 记录已应用版本，**不读写全局 `user_version` pragma**，避免与 `session-store`
//! 等其它迁移 set 在同一数据库文件上共享版本命名空间而冲突。
//!
//! 回滚策略：已存在的库在前进迁移前先 online backup 到
//! `<db>.pre-migration-v<from>.bak`，失败可经 [`DatabaseActor::restore_from`]
//! 整库回到迁移前状态。每条迁移在同一事务内执行，任一失败整批回滚。

use std::path::{Path, PathBuf};

use rusqlite::{params, OptionalExtension};

use crate::DatabaseActor;

/// 单条迁移：版本、名称、SQL（在独立事务内执行）。
#[derive(Clone, Copy, Debug)]
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// 迁移执行报告。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationReport {
    pub from_version: u32,
    pub to_version: u32,
    pub applied_versions: Vec<u32>,
    pub backup_path: Option<PathBuf>,
}

/// 迁移错误。
#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error(transparent)]
    Database(#[from] crate::DatabaseError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("schema version {found} is newer than supported {supported}")]
    UnsupportedSchema { found: u64, supported: u32 },
    #[error("migration v{version} `{name}` failed: {message}")]
    MigrationFailed {
        version: u32,
        name: String,
        message: String,
    },
    #[error("schema version {0} does not fit in u32")]
    InvalidSchemaVersion(i64),
    #[error("invalid migrations table name `{0}`")]
    InvalidTableName(String),
    #[error("duplicate migration version {version}")]
    DuplicateMigrationVersion { version: u32 },
    #[error("migration versions must be continuous from 1: expected {expected}, found {found}")]
    NonContiguousMigrationVersion { expected: u32, found: u32 },
    #[error(
        "migration plan final version {final_version:?} does not match current version {current_version}"
    )]
    MigrationPlanVersionMismatch {
        current_version: u32,
        final_version: Option<u32>,
    },
}

/// 读取命名空间内的 schema 版本（账本表不存在时返回 0）。
pub async fn schema_version(
    database: &DatabaseActor,
    migrations_table: &str,
) -> Result<u32, MigrationError> {
    validate_table_name(migrations_table)?;
    let table = migrations_table.to_string();
    let version = database
        .call(move |connection| -> rusqlite::Result<i64> {
            let exists: Option<i64> = connection
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
                    params![table],
                    |row| row.get(0),
                )
                .optional()?;
            if exists.is_none() {
                return Ok(0);
            }
            connection.query_row(
                &format!("SELECT COALESCE(MAX(version), 0) FROM {table}"),
                [],
                |row| row.get(0),
            )
        })
        .await??;
    u32::try_from(version).map_err(|_| MigrationError::InvalidSchemaVersion(version))
}

/// 在 `migrations_table` 命名空间内执行前向迁移。
///
/// - 已存在的库（`existed = true`）先备份到 `<db>.pre-migration-v<from>.bak`，
///   失败可经 [`DatabaseActor::restore_from`] 回滚到迁移前状态；
/// - 每条迁移在同一个事务内执行，任一失败整批回滚；
/// - 拒绝比 `current_version` 更新的库（不支持降级）。
pub async fn migrate(
    database: &DatabaseActor,
    migrations_table: &str,
    migrations: &[Migration],
    current_version: u32,
    database_path: &Path,
    existed: bool,
) -> Result<MigrationReport, MigrationError> {
    validate_table_name(migrations_table)?;
    validate_migration_plan(migrations, current_version)?;
    let from_version = schema_version(database, migrations_table).await?;
    if from_version > current_version {
        return Err(MigrationError::UnsupportedSchema {
            found: from_version as u64,
            supported: current_version,
        });
    }
    if from_version == current_version {
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

    let pending: Vec<(u32, String, String)> = migrations
        .iter()
        .filter(|migration| migration.version > from_version)
        .map(|migration| {
            (
                migration.version,
                migration.name.to_string(),
                migration.sql.to_string(),
            )
        })
        .collect();
    let table = migrations_table.to_string();
    let applied = database
        .call(move |connection| -> Result<Vec<u32>, MigrationError> {
            connection.execute_batch(&format!(
                "CREATE TABLE IF NOT EXISTS {table} (\
                 version INTEGER PRIMARY KEY, \
                 name TEXT NOT NULL, \
                 applied_at_ms INTEGER NOT NULL);",
            ))?;
            let transaction = connection.transaction()?;
            let mut applied = Vec::new();
            for (version, name, sql) in pending {
                if let Err(error) = transaction.execute_batch(&sql) {
                    return Err(MigrationError::MigrationFailed {
                        version,
                        name,
                        message: error.to_string(),
                    });
                }
                transaction.execute(
                    &format!(
                        "INSERT INTO {table}(version, name, applied_at_ms) \
                         VALUES (?1, ?2, CAST(strftime('%s','now') AS INTEGER) * 1000)",
                    ),
                    params![version, name],
                )?;
                applied.push(version);
            }
            transaction.commit()?;
            Ok(applied)
        })
        .await??;

    Ok(MigrationReport {
        from_version,
        to_version: current_version,
        applied_versions: applied,
        backup_path,
    })
}

fn backup_path(database_path: &Path, from_version: u32) -> PathBuf {
    let file_name = database_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("pawork.sqlite3");
    database_path.with_file_name(format!("{file_name}.pre-migration-v{from_version}.bak"))
}

fn validate_table_name(name: &str) -> Result<(), MigrationError> {
    let valid = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if valid {
        Ok(())
    } else {
        Err(MigrationError::InvalidTableName(name.to_string()))
    }
}

fn validate_migration_plan(
    migrations: &[Migration],
    current_version: u32,
) -> Result<(), MigrationError> {
    let mut seen = std::collections::HashSet::with_capacity(migrations.len());
    for migration in migrations {
        if !seen.insert(migration.version) {
            return Err(MigrationError::DuplicateMigrationVersion {
                version: migration.version,
            });
        }
    }

    for (migration, expected) in migrations.iter().zip(1_u32..) {
        if migration.version != expected {
            return Err(MigrationError::NonContiguousMigrationVersion {
                expected,
                found: migration.version,
            });
        }
    }

    let final_version = migrations.last().map(|migration| migration.version);
    let expected_final = (current_version != 0).then_some(current_version);
    if final_version != expected_final {
        return Err(MigrationError::MigrationPlanVersionMismatch {
            current_version,
            final_version,
        });
    }
    Ok(())
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
            "pawork-migration-runner-{}-{unique}-{name}",
            std::process::id()
        ))
    }

    const TEST_MIGRATIONS: &[Migration] = &[
        Migration {
            version: 1,
            name: "create_demo",
            sql: "CREATE TABLE demo(value TEXT NOT NULL);",
        },
        Migration {
            version: 2,
            name: "seed_demo",
            sql: "INSERT INTO demo(value) VALUES ('seeded');",
        },
    ];

    #[tokio::test]
    async fn fresh_database_advances_to_current_version() {
        let path = temp_path("fresh.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        let report = migrate(&actor, "demo_migrations", TEST_MIGRATIONS, 2, &path, false)
            .await
            .expect("migrate");
        assert_eq!(report.from_version, 0);
        assert_eq!(report.to_version, 2);
        assert_eq!(report.applied_versions, vec![1, 2]);
        assert!(report.backup_path.is_none());
        assert_eq!(schema_version(&actor, "demo_migrations").await.unwrap(), 2);

        let value: String = actor
            .call(|connection| connection.query_row("SELECT value FROM demo", [], |row| row.get(0)))
            .await
            .expect("actor")
            .expect("query");
        assert_eq!(value, "seeded");
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn existing_database_is_backed_up_before_migration() {
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
        let report = migrate(&actor, "demo_migrations", TEST_MIGRATIONS, 2, &path, true)
            .await
            .expect("migrate");
        let backup = report.backup_path.clone().expect("backup path");
        assert!(backup.exists());

        // 备份回滚：恢复后控制面表消失，legacy 数据保留。
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
        assert!(!tables.iter().any(|name| name == "demo"), "demo 不应残留");
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
        let error = migrate(&actor, "demo_migrations", &[bad], 1, &path, false)
            .await
            .expect_err("应迁移失败");
        assert!(
            matches!(error, MigrationError::MigrationFailed { version: 1, .. }),
            "unexpected error: {error:?}"
        );
        // 账本表仍创建，但无成功版本记录。
        assert_eq!(schema_version(&actor, "demo_migrations").await.unwrap(), 0);
        let exists: i64 = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='should_not_exist'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("actor")
            .expect("query");
        assert_eq!(exists, 0);
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn failing_v2_rolls_back_successful_v1_business_data_and_ledger() {
        let path = temp_path("failure-after-v1.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        let migrations = [
            Migration {
                version: 1,
                name: "create_and_seed_staged_demo",
                sql: "CREATE TABLE staged_demo(value TEXT NOT NULL); \
                      INSERT INTO staged_demo(value) VALUES ('v1-seeded');",
            },
            Migration {
                version: 2,
                name: "fail_after_v1",
                sql: "INSERT INTO staged_demo(value) VALUES ('v2-before-failure'); \
                      CREATE TABL invalid syntax",
            },
        ];

        let error = migrate(
            &actor,
            "atomic_batch_migrations",
            &migrations,
            2,
            &path,
            false,
        )
        .await
        .expect_err("v2 SQL 应失败");
        assert!(matches!(
            error,
            MigrationError::MigrationFailed {
                version: 2,
                ref name,
                ..
            } if name == "fail_after_v1"
        ));

        let business_tables: i64 = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM sqlite_master \
                     WHERE type='table' AND name='staged_demo'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("actor")
            .expect("query business table");
        assert_eq!(business_tables, 0, "v1 创建的业务表及其数据必须随整批回滚");

        let ledger_versions: Vec<i64> = actor
            .call(|connection| {
                let mut statement = connection
                    .prepare("SELECT version FROM atomic_batch_migrations ORDER BY version")
                    .expect("prepare ledger query");
                statement
                    .query_map([], |row| row.get(0))
                    .expect("query ledger")
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .expect("collect ledger")
            })
            .await
            .expect("actor");
        assert!(
            ledger_versions.is_empty(),
            "v1 migration ledger version 不得在 v2 失败后提交"
        );
        assert_eq!(
            schema_version(&actor, "atomic_batch_migrations")
                .await
                .expect("schema version"),
            0
        );

        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn newer_than_supported_is_rejected() {
        let path = temp_path("newer.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        // 先迁移到 v2。
        migrate(&actor, "demo_migrations", TEST_MIGRATIONS, 2, &path, false)
            .await
            .expect("seed v2");
        actor.shutdown().await.expect("shutdown");

        let actor = DatabaseActor::open(&path).await.expect("reopen");
        // 再以 current=1 尝试（库已 v2 > 1）→ 拒绝。
        let error = migrate(
            &actor,
            "demo_migrations",
            &TEST_MIGRATIONS[..1],
            1,
            &path,
            true,
        )
        .await
        .expect_err("应拒绝降级");
        assert!(
            matches!(
                error,
                MigrationError::UnsupportedSchema {
                    found: 2,
                    supported: 1
                }
            ),
            "unexpected error: {error:?}"
        );
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn namespaced_tables_do_not_collide() {
        // 命名空间隔离的是「版本账本」，不是表名：同一文件上两个独立 ledger
        // 互不共享版本号；各 set 应使用各自的表名，避免物理表冲突。
        let path = temp_path("namespaced.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        migrate(&actor, "alpha_migrations", TEST_MIGRATIONS, 2, &path, false)
            .await
            .expect("alpha");
        // beta 命名空间独立从 0 开始（账本表不同），即便 alpha 已达 v2。
        assert_eq!(schema_version(&actor, "beta_migrations").await.unwrap(), 0);
        let beta_migrations: &[Migration] = &[
            Migration {
                version: 1,
                name: "create_demo_beta",
                sql: "CREATE TABLE demo_beta(value TEXT NOT NULL);",
            },
            Migration {
                version: 2,
                name: "seed_demo_beta",
                sql: "INSERT INTO demo_beta(value) VALUES ('beta');",
            },
        ];
        let report = migrate(&actor, "beta_migrations", beta_migrations, 2, &path, true)
            .await
            .expect("beta");
        assert_eq!(report.applied_versions, vec![1, 2]);
        assert_eq!(schema_version(&actor, "alpha_migrations").await.unwrap(), 2);
        assert_eq!(schema_version(&actor, "beta_migrations").await.unwrap(), 2);
        // 两个 ledger 表共存，互不影响。
        let ledgers: i64 = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM sqlite_master \
                     WHERE type='table' AND name IN ('alpha_migrations','beta_migrations')",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("actor")
            .expect("query");
        assert_eq!(ledgers, 2);
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn invalid_table_name_is_rejected() {
        let path = temp_path("invalid.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        let error = schema_version(&actor, "bad name!")
            .await
            .expect_err("应拒绝表名");
        assert!(matches!(error, MigrationError::InvalidTableName(_)));
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn malformed_plans_fail_before_backup_or_database_write() {
        let path = temp_path("invalid-plan.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");

        let duplicate = [
            Migration {
                version: 1,
                name: "duplicate_a",
                sql: "CREATE TABLE duplicate_a(value TEXT);",
            },
            Migration {
                version: 1,
                name: "duplicate_b",
                sql: "CREATE TABLE duplicate_b(value TEXT);",
            },
        ];
        let error = migrate(
            &actor,
            "invalid_plan_migrations",
            &duplicate,
            1,
            &path,
            true,
        )
        .await
        .expect_err("重复版本应失败");
        assert!(matches!(
            error,
            MigrationError::DuplicateMigrationVersion { version: 1 }
        ));

        let gap = [
            Migration {
                version: 1,
                name: "gap_a",
                sql: "CREATE TABLE gap_a(value TEXT);",
            },
            Migration {
                version: 3,
                name: "gap_b",
                sql: "CREATE TABLE gap_b(value TEXT);",
            },
        ];
        let error = migrate(&actor, "invalid_plan_migrations", &gap, 3, &path, true)
            .await
            .expect_err("断档版本应失败");
        assert!(matches!(
            error,
            MigrationError::NonContiguousMigrationVersion {
                expected: 2,
                found: 3
            }
        ));

        let missing_final = [Migration {
            version: 1,
            name: "missing_final",
            sql: "CREATE TABLE missing_final(value TEXT);",
        }];
        let error = migrate(
            &actor,
            "invalid_plan_migrations",
            &missing_final,
            2,
            &path,
            true,
        )
        .await
        .expect_err("最终版本不存在应失败");
        assert!(matches!(
            error,
            MigrationError::MigrationPlanVersionMismatch {
                current_version: 2,
                final_version: Some(1)
            }
        ));

        let written_tables: i64 = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN (\
                     'invalid_plan_migrations','duplicate_a','duplicate_b',\
                     'gap_a','gap_b','missing_final')",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("actor")
            .expect("query");
        assert_eq!(written_tables, 0, "非法计划不得创建账本或业务表");
        assert!(
            !backup_path(&path, 0).exists(),
            "计划预检失败时不得创建备份"
        );

        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }
}
