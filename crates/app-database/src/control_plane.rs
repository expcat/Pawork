//! 控制面持久化基线迁移（P18-1，ADR-033）。
//!
//! 创建版本化、tenant-bound 的 `provider_accounts` / `credentials` 表，并种入
//! legacy 合成默认账号（tenant `local/default`、account `local/default`、principal
//! `local/user`，`ProviderAccount(default)` / `Credential(default)`，路由
//! `single_candidate`）。
//!
//! **`credentials` 表不含任何 secret 列**——API Key 存于 OS Keychain（ADR-014），
//! 此处仅记录脱敏的凭据元数据与归属关系。回退到 legacy 单凭据运行时
//! （`account-control-v1` 关闭）仍可独立工作，本迁移只提供持久化基线。
//!
//! 本模块仅依赖 app-database 的迁移运行器与 rusqlite，**不依赖 provider-control**，
//! 避免把控制面行为类型拉入存储层（依赖方向：provider-control → agent-domain，
//! app-database → rusqlite）。

use std::path::Path;

use crate::migration::{self, Migration, MigrationError, MigrationReport};
use crate::DatabaseActor;

/// 控制面 schema 当前版本（与 `provider-control` / `core-api` 对齐）。
pub const CURRENT_CONTROL_PLANE_SCHEMA_VERSION: u32 = 2;

/// 控制面迁移账本表名（独立命名空间，避免与 `session-store` 等迁移 set 冲突）。
pub const CONTROL_PLANE_MIGRATIONS_TABLE: &str = "control_plane_schema_migrations";

const CONTROL_PLANE_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "control_plane_baseline_and_synthetic_default",
        sql: r#"
            CREATE TABLE provider_accounts (
                account_id TEXT NOT NULL,
                tenant_id TEXT NOT NULL,
                provider_id TEXT NOT NULL,
                principal_id TEXT NOT NULL,
                display_name TEXT NOT NULL DEFAULT '',
                routing_strategy TEXT NOT NULL DEFAULT 'single_candidate',
                schema_version INTEGER NOT NULL CHECK (schema_version > 0),
                created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
                PRIMARY KEY (tenant_id, account_id)
            );
            CREATE INDEX idx_provider_accounts_tenant ON provider_accounts(tenant_id);
            CREATE TABLE credentials (
                credential_id TEXT NOT NULL,
                tenant_id TEXT NOT NULL,
                account_id TEXT NOT NULL,
                provider_id TEXT NOT NULL,
                credential_kind TEXT NOT NULL,
                synthetic INTEGER NOT NULL DEFAULT 0 CHECK (synthetic IN (0, 1)),
                schema_version INTEGER NOT NULL CHECK (schema_version > 0),
                created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
                PRIMARY KEY (tenant_id, credential_id),
                FOREIGN KEY (tenant_id, account_id)
                    REFERENCES provider_accounts(tenant_id, account_id) ON DELETE CASCADE
            );
            CREATE INDEX idx_credentials_account ON credentials(tenant_id, account_id);
            INSERT INTO provider_accounts
                (account_id, tenant_id, provider_id, principal_id, display_name,
                 routing_strategy, schema_version, created_at_ms)
            VALUES
                ('local/default', 'local/default', 'default', 'local/user',
                 'Legacy default account', 'single_candidate', 1, 0);
            INSERT INTO credentials
                (credential_id, tenant_id, account_id, provider_id, credential_kind,
                 synthetic, schema_version, created_at_ms)
            VALUES
                ('default', 'local/default', 'local/default', 'default', 'api_key', 1, 1, 0);
        "#,
    },
    Migration {
        version: 2,
        name: "account_credential_lifecycle_split",
        sql: r#"
            ALTER TABLE provider_accounts
                ADD COLUMN priority INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE provider_accounts
                ADD COLUMN weight INTEGER NOT NULL DEFAULT 1;
            ALTER TABLE provider_accounts
                ADD COLUMN max_concurrency INTEGER NOT NULL DEFAULT 1;
            ALTER TABLE provider_accounts
                ADD COLUMN state TEXT NOT NULL DEFAULT 'active';
            ALTER TABLE credentials
                ADD COLUMN secret_ref_service TEXT NOT NULL DEFAULT '';
            ALTER TABLE credentials
                ADD COLUMN secret_ref_account TEXT NOT NULL DEFAULT '';
            ALTER TABLE credentials
                ADD COLUMN state TEXT NOT NULL DEFAULT 'active';
            ALTER TABLE credentials
                ADD COLUMN refresh_state TEXT NOT NULL DEFAULT 'not_refreshable';
            ALTER TABLE credentials
                ADD COLUMN expires_at_ms INTEGER;
            UPDATE credentials
                SET secret_ref_service = 'default',
                    secret_ref_account = 'legacy-default'
                WHERE synthetic = 1;
        "#,
    },
];

/// 读取控制面 schema 版本（未迁移返回 0）。
pub async fn schema_version(database: &DatabaseActor) -> Result<u32, MigrationError> {
    migration::schema_version(database, CONTROL_PLANE_MIGRATIONS_TABLE).await
}

/// 执行控制面前向迁移；已存在的库先备份（rollback 基线）。
pub async fn migrate(
    database: &DatabaseActor,
    database_path: &Path,
    existed: bool,
) -> Result<MigrationReport, MigrationError> {
    migration::migrate(
        database,
        CONTROL_PLANE_MIGRATIONS_TABLE,
        CONTROL_PLANE_MIGRATIONS,
        CURRENT_CONTROL_PLANE_SCHEMA_VERSION,
        database_path,
        existed,
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use rusqlite::params;

    use super::*;

    // 冻结的 legacy 作用域字符串（与 provider-control `legacy`、core-api 常量一致；
    // 由基线 SQL 种入，测试据此断言）。
    const LEGACY_TENANT: &str = "local/default";
    const LEGACY_ACCOUNT: &str = "local/default";
    const LEGACY_PRINCIPAL: &str = "local/user";
    const LEGACY_PROVIDER: &str = "default";
    const LEGACY_CREDENTIAL: &str = "default";
    const LEGACY_ROUTING: &str = "single_candidate";
    const LEGACY_CREDENTIAL_KIND: &str = "api_key";

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_path(name: &str) -> PathBuf {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pawork-control-plane-migration-{}-{unique}-{name}",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn baseline_seeds_synthetic_default_at_legacy_scope() {
        let path = temp_path("baseline.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        let report = migrate(&actor, &path, false).await.expect("migrate");
        assert_eq!(report.from_version, 0);
        assert_eq!(report.to_version, CURRENT_CONTROL_PLANE_SCHEMA_VERSION);
        assert_eq!(report.applied_versions, vec![1, 2]);
        assert_eq!(
            schema_version(&actor).await.unwrap(),
            CURRENT_CONTROL_PLANE_SCHEMA_VERSION
        );

        let account: (String, String, String, String, String) = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT account_id, tenant_id, provider_id, principal_id, routing_strategy \
                     FROM provider_accounts WHERE tenant_id='local/default'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
            })
            .await
            .expect("actor")
            .expect("account row");
        assert_eq!(account.0, LEGACY_ACCOUNT);
        assert_eq!(account.1, LEGACY_TENANT);
        assert_eq!(account.2, LEGACY_PROVIDER);
        assert_eq!(account.3, LEGACY_PRINCIPAL);
        assert_eq!(account.4, LEGACY_ROUTING);

        let credential: (String, String, String, String, i64) = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT credential_id, account_id, provider_id, credential_kind, synthetic \
                     FROM credentials WHERE tenant_id='local/default'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    },
                )
            })
            .await
            .expect("actor")
            .expect("credential row");
        assert_eq!(credential.0, LEGACY_CREDENTIAL);
        assert_eq!(credential.1, LEGACY_ACCOUNT);
        assert_eq!(credential.2, LEGACY_PROVIDER);
        assert_eq!(credential.3, LEGACY_CREDENTIAL_KIND);
        assert_eq!(credential.4, 1, "合成凭据应标记 synthetic=1");

        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn credentials_table_has_no_secret_columns() {
        // ADR-014：secret 不入库。introspect pragma table_info，断言无 secret 类列。
        let path = temp_path("nosecret.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        migrate(&actor, &path, false).await.expect("migrate");
        let columns: Vec<String> = actor
            .call(|connection| {
                let mut statement = connection
                    .prepare("PRAGMA table_info(credentials)")
                    .expect("prepare");
                statement
                    .query_map([], |row| row.get::<_, String>(1))
                    .expect("query")
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .expect("collect")
            })
            .await
            .expect("actor");
        for forbidden in [
            "secret",
            "token",
            "api_key",
            "apikey",
            "password",
            "credential_value",
            "value",
        ] {
            assert!(
                !columns.iter().any(|column| column == forbidden),
                "credentials 表不得包含列 `{forbidden}`，实际列: {columns:?}"
            );
        }
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn rollback_via_backup_removes_control_plane_tables() {
        let path = temp_path("rollback.sqlite3");
        // 预置 legacy 数据，existed=true → 备份。
        let legacy = DatabaseActor::open(&path).await.expect("legacy open");
        legacy
            .call(|connection| {
                connection.execute_batch(
                    "CREATE TABLE legacy_marker(value TEXT); \
                     INSERT INTO legacy_marker VALUES ('kept');",
                )
            })
            .await
            .expect("actor")
            .expect("seed");
        legacy.shutdown().await.expect("legacy shutdown");

        let actor = DatabaseActor::open(&path).await.expect("reopen");
        let report = migrate(&actor, &path, true).await.expect("migrate");
        let backup = report.backup_path.clone().expect("backup path");
        assert!(backup.exists());

        // 回滚：恢复备份后控制面表消失，legacy 数据保留。
        actor.restore_from(&backup).await.expect("restore");
        let control_plane_present: i64 = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
                     AND name IN ('provider_accounts','credentials','control_plane_schema_migrations')",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("actor")
            .expect("query");
        assert_eq!(control_plane_present, 0, "回滚后控制面表必须全部消失");
        let legacy_value: String = actor
            .call(|connection| {
                connection.query_row("SELECT value FROM legacy_marker", [], |row| row.get(0))
            })
            .await
            .expect("actor")
            .expect("query");
        assert_eq!(legacy_value, "kept");
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(backup);
    }

    #[tokio::test]
    async fn re_migrate_is_idempotent() {
        let path = temp_path("idempotent.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        let first = migrate(&actor, &path, false).await.expect("first migrate");
        assert_eq!(first.applied_versions, vec![1, 2]);
        // 第二次：已是当前版本，不应再应用，不应再备份。
        let second = migrate(&actor, &path, true).await.expect("second migrate");
        assert!(second.applied_versions.is_empty());
        assert!(second.backup_path.is_none());
        assert_eq!(
            schema_version(&actor).await.unwrap(),
            CURRENT_CONTROL_PLANE_SCHEMA_VERSION
        );
        // 合成行不被重复插入（PRIMARY KEY 约束 + 幂等路径）。
        let account_count: i64 = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM provider_accounts WHERE tenant_id='local/default'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("actor")
            .expect("query");
        assert_eq!(account_count, 1);
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn cascade_delete_drops_credentials_with_account() {
        // 验证外键 ON DELETE CASCADE 在控制面 schema 上正常工作（tenant boundary 完整性）。
        let path = temp_path("cascade.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        migrate(&actor, &path, false).await.expect("migrate");
        actor
            .call(|connection| {
                connection.execute(
                    "DELETE FROM provider_accounts WHERE tenant_id='local/default' AND account_id='local/default'",
                    [],
                )
            })
            .await
            .expect("actor")
            .expect("delete");
        let remaining: i64 = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM credentials WHERE tenant_id='local/default'",
                    params![],
                    |row| row.get(0),
                )
            })
            .await
            .expect("actor")
            .expect("query");
        assert_eq!(remaining, 0, "账号删除后凭据应级联删除");
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn v2_upgrade_preserves_synthetic_and_backfills_secret_ref() {
        // P18-3：v1 → v2 升级后，合成账号/凭据仍存在，secret_ref 被回灌为 sentinel，
        // 新增账号侧 priority/weight/max_concurrency/state 默认值正确。
        let path = temp_path("v2-upgrade.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        let report = migrate(&actor, &path, false).await.expect("migrate");
        assert_eq!(report.from_version, 0);
        assert_eq!(report.to_version, 2);
        assert_eq!(report.applied_versions, vec![1, 2]);

        // 账号新增字段默认值。
        let account: (i64, i64, i64, String) = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT priority, weight, max_concurrency, state \
                     FROM provider_accounts WHERE tenant_id='local/default'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
            })
            .await
            .expect("actor")
            .expect("account row");
        assert_eq!(account.0, 0, "priority 默认 0");
        assert_eq!(account.1, 1, "weight 默认 1");
        assert_eq!(account.2, 1, "max_concurrency 默认 1");
        assert_eq!(account.3, "active", "state 默认 active");

        // 合成凭据 secret_ref 回灌为 sentinel (`default`, `legacy-default`)。
        let credential: (String, String, String, String, Option<i64>) = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT secret_ref_service, secret_ref_account, state, refresh_state, \
                     expires_at_ms FROM credentials WHERE tenant_id='local/default'",
                    [],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
            })
            .await
            .expect("actor")
            .expect("credential row");
        assert_eq!(credential.0, "default", "secret_ref_service 回灌 sentinel");
        assert_eq!(
            credential.1, "legacy-default",
            "secret_ref_account 回灌 sentinel"
        );
        assert_eq!(credential.2, "active", "credential state 默认 active");
        assert_eq!(
            credential.3, "not_refreshable",
            "refresh_state 默认 not_refreshable"
        );
        assert!(credential.4.is_none(), "expires_at_ms 默认 NULL");

        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn v2_credentials_table_still_has_no_plaintext_columns() {
        // P18-3 安全红线（ADR-014）：v2 新增的 secret_ref_service / secret_ref_account
        // 是 opaque 定位对，绝非明文列。introspect table_info 断言无 secret 类列。
        let path = temp_path("v2-nosecret.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        migrate(&actor, &path, false).await.expect("migrate");
        let columns: Vec<String> = actor
            .call(|connection| {
                let mut statement = connection
                    .prepare("PRAGMA table_info(credentials)")
                    .expect("prepare");
                statement
                    .query_map([], |row| row.get::<_, String>(1))
                    .expect("query")
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .expect("collect")
            })
            .await
            .expect("actor");
        for forbidden in [
            "secret",
            "token",
            "api_key",
            "apikey",
            "password",
            "credential_value",
            "value",
        ] {
            assert!(
                !columns.iter().any(|column| column == forbidden),
                "credentials 表不得包含明文列 `{forbidden}`，实际列: {columns:?}"
            );
        }
        // secret_ref_service / secret_ref_account 作为 opaque 定位存在。
        assert!(columns.iter().any(|c| c == "secret_ref_service"));
        assert!(columns.iter().any(|c| c == "secret_ref_account"));
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn cross_tenant_queries_are_isolated() {
        // P18-3：tenant scope 隔离——不同 tenant 的账号/凭据互不可见。
        let path = temp_path("cross-tenant.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        migrate(&actor, &path, false).await.expect("migrate");
        // 种入第二个 tenant 的账号与凭据。
        actor
            .call(|connection| {
                connection.execute_batch(
                    "INSERT INTO provider_accounts(account_id, tenant_id, provider_id, \
                     principal_id, display_name, routing_strategy, schema_version, \
                     created_at_ms) VALUES ('acct-x', 'tenant-x', 'openai', 'p-x', \
                     'X account', 'single_candidate', 2, 0); \
                     INSERT INTO credentials(credential_id, tenant_id, account_id, \
                     provider_id, credential_kind, synthetic, schema_version, \
                     created_at_ms, secret_ref_service, secret_ref_account, state) \
                     VALUES ('cred-x', 'tenant-x', 'acct-x', 'openai', 'api_key', 0, \
                     2, 0, 'pawork.openai', 'acct-x', 'active');",
                )
            })
            .await
            .expect("actor")
            .expect("seed tenant-x");

        // tenant-x 的账号存在。
        let tenant_x_account: i64 = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM provider_accounts WHERE tenant_id='tenant-x'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("actor")
            .expect("query");
        assert_eq!(tenant_x_account, 1);
        // local/default 的账号（synthetic）与 tenant-x 互不干扰。
        let local_account: i64 = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM provider_accounts WHERE tenant_id='local/default'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("actor")
            .expect("query");
        assert_eq!(local_account, 1);
        // tenant-x 凭据 secret_ref 不被 local/default 合成凭据影响。
        let tenant_x_secret: (String, String) = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT secret_ref_service, secret_ref_account FROM credentials \
                     WHERE tenant_id='tenant-x'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .await
            .expect("actor")
            .expect("query");
        assert_eq!(tenant_x_secret.0, "pawork.openai");
        assert_eq!(tenant_x_secret.1, "acct-x");
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }
}
