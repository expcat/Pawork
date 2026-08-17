//! Credential-lease 持久化投影与迁移（P18-4，ADR-016/033）。
//!
//! 创建版本化、tenant-bound 的 `credential_leases` 表，存储 canonical lease 快照
//! （`Requested/Acquired/Released/Expired/Reclaimed` 状态机，与 `provider-control::lease`
//! 对齐），用于崩溃 / 重启后的恢复扫描。
//!
//! **本表不含任何 secret 列**：lease 只携带定位 / 归属 / 期限信息；明文 API Key
//! 由 OS Keychain（ADR-014）在 lease 之外解析。`credential_id` 是 opaque 定位符，
//! 非明文。
//!
//! 本模块只持久化扁平行（`pawork-sqlite` + rusqlite）；领域 [`crate::LeaseRecord`]
//! 由组合层在两侧间转换。DDL 轴与记录版本轴是两套数字，不要合并。

use std::path::Path;

use rusqlite::Connection;

use pawork_sqlite::{
    migrate as run_migrate, schema_version as read_schema_version, DatabaseActor, DatabaseError,
    Migration, MigrationError, MigrationReport,
};

/// Lease 投影**迁移账本**当前版本。
///
/// 该值表示 SQLite DDL 的迁移序号，与 canonical lease 记录携带的
/// `crate::lease::LEASE_SCHEMA_VERSION` 是两个独立版本轴，数值无需相同。
pub const CURRENT_LEASE_SCHEMA_VERSION: u32 = 3;

/// Lease 投影迁移账本表名（独立命名空间）。
pub const LEASE_MIGRATIONS_TABLE: &str = "credential_leases_schema_migrations";

/// `credential_leases` 业务表名。
pub const CREDENTIAL_LEASES_TABLE: &str = "credential_leases";

/// `credential_lease_events` append-only 事件日志表名。
pub const CREDENTIAL_LEASE_EVENTS_TABLE: &str = "credential_lease_events";

/// 未知 / 已 GC 的 lease 行查询结果（调用方据此判定 `already_released`）。
#[derive(Debug, thiserror::Error)]
pub enum LeaseProjectionError {
    /// 底层 SQLite 错误。
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    /// 数据库 Actor 已关闭。
    #[error(transparent)]
    Database(#[from] DatabaseError),
}

const LEASE_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "credential_leases_baseline",
        sql: r#"
            CREATE TABLE credential_leases (
                lease_id TEXT NOT NULL,
                schema_version INTEGER NOT NULL CHECK (schema_version > 0),
                version INTEGER NOT NULL CHECK (version > 0),
                state TEXT NOT NULL CHECK (
                    state IN ('requested','acquired','released','expired','reclaimed')
                ),
                tenant_id TEXT NOT NULL,
                account_id TEXT NOT NULL,
                provider_id TEXT NOT NULL,
                credential_id TEXT NOT NULL,
                principal_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                acquired_at_ms INTEGER NOT NULL CHECK (acquired_at_ms >= 0),
                ttl_ms INTEGER NOT NULL CHECK (ttl_ms >= 0),
                expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms >= 0),
                outcome TEXT,
                trace_id TEXT,
                PRIMARY KEY (lease_id)
            );
            CREATE INDEX idx_credential_leases_tenant
                ON credential_leases(tenant_id);
            CREATE INDEX idx_credential_leases_account
                ON credential_leases(tenant_id, account_id);
            CREATE INDEX idx_credential_leases_state
                ON credential_leases(state);
        "#,
    },
    Migration {
        version: 2,
        name: "credential_leases_outstanding_partial_index",
        // 为 `load_outstanding`（非终态 lease）提供高效局部索引；终态行（reclaimed）
        // 不进索引，加快恢复扫描。该 index 在 v1 已可工作，v2 仅作为版本化占位
        // 以保留未来 schema 演进空间（如增加 reclaim 原因列）。
        sql: r#"
            CREATE INDEX IF NOT EXISTS idx_credential_leases_outstanding
                ON credential_leases(state)
                WHERE state IN ('requested','acquired','released','expired');
        "#,
    },
    Migration {
        version: 3,
        name: "credential_lease_events_append_log",
        // append-only 事件日志（ADR-016/033）：每个状态转换产生的事件按序追加，
        // 与 `credential_leases` 快照分离。事件永不被 settle 删除（审计 / 重放）。
        // `payload` 为调用方序列化的 opaque JSON（app-database 不依赖 provider-control，
        // 不解析事件结构）。lease 行被 settle 删除后，事件行保留。
        sql: r#"
            CREATE TABLE credential_lease_events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                lease_id TEXT NOT NULL,
                version INTEGER NOT NULL CHECK (version > 0),
                kind TEXT NOT NULL CHECK (
                    kind IN ('requested','acquired','released','expired','reclaimed')
                ),
                payload TEXT NOT NULL,
                at_ms INTEGER NOT NULL CHECK (at_ms >= 0)
            );
            CREATE INDEX idx_credential_lease_events_lease
                ON credential_lease_events(lease_id);
        "#,
    },
];

/// 读取 lease 投影 schema 版本（未迁移返回 0）。
pub async fn schema_version(database: &DatabaseActor) -> Result<u32, MigrationError> {
    read_schema_version(database, LEASE_MIGRATIONS_TABLE).await
}

/// 执行 lease 投影前向迁移；已存在的库先备份（rollback 基线）。
pub async fn migrate(
    database: &DatabaseActor,
    database_path: &Path,
    existed: bool,
) -> Result<MigrationReport, MigrationError> {
    run_migrate(
        database,
        LEASE_MIGRATIONS_TABLE,
        LEASE_MIGRATIONS,
        CURRENT_LEASE_SCHEMA_VERSION,
        database_path,
        existed,
    )
    .await
}

/// `credential_leases` 表的列集合（供 `no-secret` 自省断言复用）。
pub const EXPECTED_COLUMNS: &[&str] = &[
    "lease_id",
    "schema_version",
    "version",
    "state",
    "tenant_id",
    "account_id",
    "provider_id",
    "credential_id",
    "principal_id",
    "agent_id",
    "session_id",
    "acquired_at_ms",
    "ttl_ms",
    "expires_at_ms",
    "outcome",
    "trace_id",
];

/// 列出 `credential_leases` 表的实际列名（按声明顺序）。表不存在时返回空 Vec。
pub async fn columns(database: &DatabaseActor) -> Result<Vec<String>, LeaseProjectionError> {
    Ok(database
        .call(|connection| -> rusqlite::Result<Vec<String>> {
            let exists: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    rusqlite::params![CREDENTIAL_LEASES_TABLE],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            if exists == 0 {
                return Ok(Vec::new());
            }
            let mut statement =
                connection.prepare(&format!("PRAGMA table_info({CREDENTIAL_LEASES_TABLE})"))?;
            let names = statement
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(names)
        })
        .await??)
}

/// 读取非终态 lease 行数（恢复扫描负载指标；表不存在返回 0）。
pub async fn outstanding_count(database: &DatabaseActor) -> Result<u64, LeaseProjectionError> {
    let count: i64 = database
        .call(|connection| -> rusqlite::Result<i64> {
            connection.query_row(
                &format!(
                    "SELECT COUNT(*) FROM {CREDENTIAL_LEASES_TABLE} \
                     WHERE state IN ('requested','acquired','released','expired')"
                ),
                [],
                |row| row.get(0),
            )
        })
        .await?
        // 表不存在（migrate 未运行）时 COUNT 失败 -> 归一为 0，不报错。
        .unwrap_or(0);
    Ok(count.max(0) as u64)
}

// ---------------------------------------------------------------------------
// Durable row repository（P18-4 #4：扁平、无 secret 的可持久化行仓库）
// ---------------------------------------------------------------------------
//
// 本仓库是「存储原语」，不依赖 `provider-control`：宿主组合层（core-runtime /
// app-service）负责把 canonical `LeaseRecord` / `LeaseEvent` 映射到下方扁平行，
// 并把 `LeaseEvent` 序列化为 opaque JSON `payload` 串。app-database 只忠实持久化
// 这些行 / 串，不解析事件结构，也不实现 `LeaseProjection` trait（trait 适配由
// 组合层完成，避免存储层反向依赖控制面）。
//
// 所有写操作经 `DatabaseActor::call` 在 Actor 串行线程上执行；`apply` 在单个
// SQLite 事务内 upsert 快照 + 追加事件，保证快照与事件 all-or-nothing（崩溃后
// 不会出现「快照已写但事件丢失」的撕裂状态）。

/// `credential_leases` 快照行的扁平 DTO（与表列一一对应，无 secret 列）。
///
/// `state` / `outcome` 以 DB 对齐字符串存储（`"requested"`/`"acquired"`/...；
/// outcome 为 `LeaseOutcome` 的 db_str 或 `None`），由组合层与
/// `provider-control::lease::LeaseState` / `LeaseOutcome` 互转。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseSnapshotRow {
    /// lease 唯一标识（与事件行的 `lease_id` 对齐）。
    pub lease_id: String,
    /// 实体 schema 版本（= `CURRENT_LEASE_SCHEMA_VERSION`）。
    pub schema_version: u32,
    /// 乐观并发版本号（每次状态转换 +1）。
    pub version: u64,
    /// 当前状态（`"requested"|"acquired"|"released"|"expired"|"reclaimed"`）。
    pub state: String,
    /// 所属租户。
    pub tenant_id: String,
    /// 被占用的账号。
    pub account_id: String,
    /// 使用的 Provider。
    pub provider_id: String,
    /// 绑定的凭据定位符（opaque，非明文 secret）。
    pub credential_id: String,
    /// 发起主体（ownership）。
    pub principal_id: String,
    /// 持有 lease 的 Agent。
    pub agent_id: String,
    /// 持有 lease 的会话。
    pub session_id: String,
    /// 授予时刻（Unix 毫秒）。
    pub acquired_at_ms: u64,
    /// TTL（毫秒）。
    pub ttl_ms: u64,
    /// 过期时刻（Unix 毫秒）。
    pub expires_at_ms: u64,
    /// 释放结果分类（db_str 或 `None`）。
    pub outcome: Option<String>,
    /// 可选追踪标识。
    pub trace_id: Option<String>,
}

/// `credential_lease_events` append-only 事件行的扁平 DTO。
///
/// `kind` 与快照 `state` 同词表；`payload` 为调用方序列化的 opaque JSON（仓库
/// 不解析）。插入时 `seq` 留 `None`，由 SQLite `AUTOINCREMENT` 赋值；读回时为
/// `Some(seq)`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseEventRow {
    /// 自增主键；插入时传 `None`，读回时为 `Some`。
    pub seq: Option<i64>,
    /// 所属 lease。
    pub lease_id: String,
    /// 事件携带的 version（转换后的新 version）。
    pub version: u64,
    /// 事件类型（`"requested"|"acquired"|"released"|"expired"|"reclaimed"`）。
    pub kind: String,
    /// opaque JSON 载荷（调用方序列化，仓库不解析）。
    pub payload: String,
    /// 事件发生时刻（Unix 毫秒）。
    pub at_ms: u64,
}

/// 绑定单个 `DatabaseActor` 的 lease 行仓库（生产存储原语）。
///
/// 不实现 `LeaseProjection`：那是组合层的职责（在 `provider-control` trait 与
/// 本仓库间适配）。本类型只提供事务化 CRUD / load / settle / append-event。
#[derive(Clone)]
pub struct LeaseRowRepository {
    database: DatabaseActor,
}

impl LeaseRowRepository {
    /// 以一个 Actor 构造仓库（`DatabaseActor` 内部为 `Arc`，clone 廉价）。
    pub fn new(database: DatabaseActor) -> Self {
        Self { database }
    }

    /// 借用底层 Actor（供组合层复用同一连接做迁移等操作）。
    pub fn actor(&self) -> &DatabaseActor {
        &self.database
    }

    /// 单事务持久化：upsert 快照 + 追加全部事件（all-or-nothing）。
    ///
    /// 任一写入失败（如事件 `kind` 违反 CHECK 约束）→ 整事务回滚，快照与事件
    /// 都不残留。这是 `LeaseProjection::apply` 所需的原子性保证。
    pub async fn apply(
        &self,
        snapshot: &LeaseSnapshotRow,
        events: &[LeaseEventRow],
    ) -> Result<(), LeaseProjectionError> {
        let snapshot = snapshot.clone();
        let events = events.to_vec();
        self.database
            .call(move |connection| -> rusqlite::Result<()> {
                let transaction = connection.transaction()?;
                upsert_snapshot_on(&transaction, &snapshot)?;
                for event in &events {
                    append_event_on(&transaction, event)?;
                }
                transaction.commit()?;
                Ok(())
            })
            .await??;
        Ok(())
    }

    /// 单独 upsert 一个快照行（非事务组合的便捷入口）。
    pub async fn upsert_snapshot(
        &self,
        row: &LeaseSnapshotRow,
    ) -> Result<(), LeaseProjectionError> {
        let row = row.clone();
        self.database
            .call(move |connection| -> rusqlite::Result<()> {
                upsert_snapshot_on(connection, &row)
            })
            .await??;
        Ok(())
    }

    /// 单独追加一个事件行（非事务组合的便捷入口）。
    pub async fn append_event(&self, row: &LeaseEventRow) -> Result<(), LeaseProjectionError> {
        let row = row.clone();
        self.database
            .call(move |connection| -> rusqlite::Result<()> { append_event_on(connection, &row) })
            .await??;
        Ok(())
    }

    /// 读取所有非终态（`requested/acquired/released/expired`）快照，供启动恢复扫描。
    pub async fn load_outstanding(&self) -> Result<Vec<LeaseSnapshotRow>, LeaseProjectionError> {
        Ok(self
            .database
            .call(|connection| -> rusqlite::Result<Vec<LeaseSnapshotRow>> {
                let mut statement = connection.prepare(
                    "SELECT lease_id, schema_version, version, state, tenant_id, account_id, \
                     provider_id, credential_id, principal_id, agent_id, session_id, \
                     acquired_at_ms, ttl_ms, expires_at_ms, outcome, trace_id \
                     FROM credential_leases \
                     WHERE state IN ('requested','acquired','released','expired')",
                )?;
                let rows = statement.query_map([], map_snapshot_row)?;
                rows.collect()
            })
            .await??)
    }

    /// 读取某租户下所有非终态快照（tenant-scoped 恢复，支持租户隔离重建）。
    pub async fn load_outstanding_for_tenant(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<LeaseSnapshotRow>, LeaseProjectionError> {
        let tenant_id = tenant_id.to_string();
        Ok(self
            .database
            .call(
                move |connection| -> rusqlite::Result<Vec<LeaseSnapshotRow>> {
                    let mut statement = connection.prepare(
                        "SELECT lease_id, schema_version, version, state, tenant_id, account_id, \
                     provider_id, credential_id, principal_id, agent_id, session_id, \
                     acquired_at_ms, ttl_ms, expires_at_ms, outcome, trace_id \
                     FROM credential_leases \
                     WHERE tenant_id = ?1 \
                     AND state IN ('requested','acquired','released','expired')",
                    )?;
                    let rows =
                        statement.query_map(rusqlite::params![&tenant_id], map_snapshot_row)?;
                    rows.collect()
                },
            )
            .await??)
    }

    /// 读取单个 lease 快照（任意状态）；不存在返回 `None`。
    pub async fn load(
        &self,
        lease_id: &str,
    ) -> Result<Option<LeaseSnapshotRow>, LeaseProjectionError> {
        let lease_id = lease_id.to_string();
        Ok(self
            .database
            .call(
                move |connection| -> rusqlite::Result<Option<LeaseSnapshotRow>> {
                    let mut statement = connection.prepare(
                        "SELECT lease_id, schema_version, version, state, tenant_id, account_id, \
                     provider_id, credential_id, principal_id, agent_id, session_id, \
                     acquired_at_ms, ttl_ms, expires_at_ms, outcome, trace_id \
                     FROM credential_leases WHERE lease_id = ?1",
                    )?;
                    let mut rows =
                        statement.query_map(rusqlite::params![&lease_id], map_snapshot_row)?;
                    match rows.next() {
                        Some(row) => Ok(Some(row?)),
                        None => Ok(None),
                    }
                },
            )
            .await??)
    }

    /// 结算：删除快照行（移出活跃集合），事件日志保留。幂等：行不存在亦返回 `Ok`。
    pub async fn settle(&self, lease_id: &str) -> Result<(), LeaseProjectionError> {
        let lease_id = lease_id.to_string();
        self.database
            .call(move |connection| -> rusqlite::Result<()> {
                connection.execute(
                    "DELETE FROM credential_leases WHERE lease_id = ?1",
                    rusqlite::params![&lease_id],
                )?;
                Ok(())
            })
            .await??;
        Ok(())
    }

    /// 读取某 lease 的全部事件（按自增 `seq` 升序），供审计 / 重放。
    pub async fn load_events(
        &self,
        lease_id: &str,
    ) -> Result<Vec<LeaseEventRow>, LeaseProjectionError> {
        let lease_id = lease_id.to_string();
        Ok(self
            .database
            .call(move |connection| -> rusqlite::Result<Vec<LeaseEventRow>> {
                let mut statement = connection.prepare(
                    "SELECT seq, lease_id, version, kind, payload, at_ms \
                     FROM credential_lease_events WHERE lease_id = ?1 ORDER BY seq",
                )?;
                let rows = statement.query_map(rusqlite::params![&lease_id], |row| {
                    Ok(LeaseEventRow {
                        seq: Some(row.get::<_, i64>(0)?),
                        lease_id: row.get::<_, String>(1)?,
                        version: row.get::<_, i64>(2)? as u64,
                        kind: row.get::<_, String>(3)?,
                        payload: row.get::<_, String>(4)?,
                        at_ms: row.get::<_, i64>(5)? as u64,
                    })
                })?;
                rows.collect()
            })
            .await??)
    }

    /// 事件日志总条数（append-only，断言「事件不再被丢弃」用）。
    pub async fn event_count(&self) -> Result<u64, LeaseProjectionError> {
        let count: i64 = self
            .database
            .call(|connection| -> rusqlite::Result<i64> {
                connection.query_row("SELECT COUNT(*) FROM credential_lease_events", [], |row| {
                    row.get(0)
                })
            })
            .await?
            // 表不存在（未迁移）时归一为 0。
            .unwrap_or(0);
        Ok(count.max(0) as u64)
    }
}

/// 在给定连接 / 事务上 upsert 一个快照行（内部复用，供 `apply` 与独立入口共享）。
fn upsert_snapshot_on(connection: &Connection, row: &LeaseSnapshotRow) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO credential_leases(\
         lease_id, schema_version, version, state, tenant_id, account_id, \
         provider_id, credential_id, principal_id, agent_id, session_id, \
         acquired_at_ms, ttl_ms, expires_at_ms, outcome, trace_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16) \
         ON CONFLICT(lease_id) DO UPDATE SET \
         schema_version=excluded.schema_version, version=excluded.version, \
         state=excluded.state, tenant_id=excluded.tenant_id, \
         account_id=excluded.account_id, provider_id=excluded.provider_id, \
         credential_id=excluded.credential_id, principal_id=excluded.principal_id, \
         agent_id=excluded.agent_id, session_id=excluded.session_id, \
         acquired_at_ms=excluded.acquired_at_ms, ttl_ms=excluded.ttl_ms, \
         expires_at_ms=excluded.expires_at_ms, outcome=excluded.outcome, \
         trace_id=excluded.trace_id",
        rusqlite::params![
            row.lease_id.as_str(),
            row.schema_version as i64,
            row.version as i64,
            row.state.as_str(),
            row.tenant_id.as_str(),
            row.account_id.as_str(),
            row.provider_id.as_str(),
            row.credential_id.as_str(),
            row.principal_id.as_str(),
            row.agent_id.as_str(),
            row.session_id.as_str(),
            row.acquired_at_ms as i64,
            row.ttl_ms as i64,
            row.expires_at_ms as i64,
            row.outcome.as_deref(),
            row.trace_id.as_deref(),
        ],
    )?;
    Ok(())
}

/// 在给定连接 / 事务上追加一个事件行（内部复用）。`seq` 由 AUTOINCREMENT 赋值。
fn append_event_on(connection: &Connection, row: &LeaseEventRow) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO credential_lease_events(\
         lease_id, version, kind, payload, at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            row.lease_id.as_str(),
            row.version as i64,
            row.kind.as_str(),
            row.payload.as_str(),
            row.at_ms as i64,
        ],
    )?;
    Ok(())
}

/// `query_map` 闭包：把一行 `credential_leases` 映射为 `LeaseSnapshotRow`。
fn map_snapshot_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LeaseSnapshotRow> {
    Ok(LeaseSnapshotRow {
        lease_id: row.get::<_, String>(0)?,
        schema_version: row.get::<_, i64>(1)? as u32,
        version: row.get::<_, i64>(2)? as u64,
        state: row.get::<_, String>(3)?,
        tenant_id: row.get::<_, String>(4)?,
        account_id: row.get::<_, String>(5)?,
        provider_id: row.get::<_, String>(6)?,
        credential_id: row.get::<_, String>(7)?,
        principal_id: row.get::<_, String>(8)?,
        agent_id: row.get::<_, String>(9)?,
        session_id: row.get::<_, String>(10)?,
        acquired_at_ms: row.get::<_, i64>(11)? as u64,
        ttl_ms: row.get::<_, i64>(12)? as u64,
        expires_at_ms: row.get::<_, i64>(13)? as u64,
        outcome: row.get::<_, Option<String>>(14)?,
        trace_id: row.get::<_, Option<String>>(15)?,
    })
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
            "pawork-lease-projection-{}-{unique}-{name}",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn fresh_database_advances_to_current_version() {
        let path = temp_path("fresh.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        let report = migrate(&actor, &path, false).await.expect("migrate");
        assert_eq!(report.from_version, 0);
        assert_eq!(report.to_version, CURRENT_LEASE_SCHEMA_VERSION);
        assert_eq!(report.applied_versions, vec![1, 2, 3]);
        assert_eq!(
            schema_version(&actor).await.unwrap(),
            CURRENT_LEASE_SCHEMA_VERSION
        );
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn re_migrate_is_idempotent() {
        let path = temp_path("idempotent.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        migrate(&actor, &path, false).await.expect("first");
        let second = migrate(&actor, &path, true).await.expect("second");
        assert!(second.applied_versions.is_empty());
        assert!(second.backup_path.is_none());
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn lease_table_has_no_secret_columns() {
        // ADR-014：secret 不入库。introspect pragma table_info，断言无 secret 类列。
        let path = temp_path("nosecret.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        migrate(&actor, &path, false).await.expect("migrate");
        let cols = columns(&actor).await.expect("columns");
        for forbidden in [
            "secret",
            "token",
            "api_key",
            "apikey",
            "password",
            "secret_ref",
            "value",
            "plaintext",
        ] {
            assert!(
                !cols.iter().any(|c| c == forbidden),
                "credential_leases 不得包含列 `{forbidden}`，实际列: {cols:?}"
            );
        }
        // credential_id 是 opaque 定位符，允许存在。
        assert!(cols.iter().any(|c| c == "credential_id"));
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn state_check_constraint_rejects_invalid_values() {
        // CHECK 约束：state 必须在白名单内。
        let path = temp_path("check.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        migrate(&actor, &path, false).await.expect("migrate");
        let result: rusqlite::Result<()> = actor
            .call(|connection| {
                connection
                    .execute(
                        "INSERT INTO credential_leases(lease_id, schema_version, version, \
                         state, tenant_id, account_id, provider_id, credential_id, \
                         principal_id, agent_id, session_id, acquired_at_ms, ttl_ms, \
                         expires_at_ms) \
                         VALUES ('l1', 2, 2, 'bogus', 't', 'a', 'p', 'c', 'pr', 'ag', \
                         's', 0, 0, 0)",
                        [],
                    )
                    .map(|_| ())
            })
            .await
            .expect("actor");
        assert!(result.is_err(), "CHECK 约束应拒绝非法 state 值");
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn rollback_via_backup_removes_lease_tables() {
        let path = temp_path("rollback.sqlite3");
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
        actor.restore_from(&backup).await.expect("restore");

        let present: i64 = actor
            .call(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
                    AND name IN ('credential_leases','credential_lease_events',\
                    'credential_leases_schema_migrations')",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("actor")
            .expect("query");
        assert_eq!(present, 0, "回滚后 lease 表必须全部消失");
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(backup);
    }

    #[tokio::test]
    async fn failed_migration_rolls_back_batch() {
        // v2 故意失败，验证整批回滚（v1 表不残留）。
        let path = temp_path("failure.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        let bad = [
            Migration {
                version: 1,
                name: "create_leases",
                sql: "CREATE TABLE credential_leases(value TEXT);",
            },
            Migration {
                version: 2,
                name: "fail",
                sql: "CREATE TABL invalid syntax",
            },
        ];
        let error = pawork_sqlite::migrate(&actor, LEASE_MIGRATIONS_TABLE, &bad, 2, &path, false)
            .await
            .expect_err("v2 SQL 应失败");
        assert!(matches!(
            error,
            MigrationError::MigrationFailed { version: 2, .. }
        ));
        assert_eq!(schema_version(&actor).await.unwrap(), 0);
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    // ---- LeaseRowRepository（P18-4 #4）----------------------------------------

    fn sample_snapshot(lease_id: &str, tenant: &str, state: &str) -> LeaseSnapshotRow {
        LeaseSnapshotRow {
            lease_id: lease_id.to_string(),
            schema_version: CURRENT_LEASE_SCHEMA_VERSION,
            version: 2,
            state: state.to_string(),
            tenant_id: tenant.to_string(),
            account_id: "acct-1".to_string(),
            provider_id: "prov-1".to_string(),
            credential_id: "cred-1".to_string(),
            principal_id: "prin-1".to_string(),
            agent_id: "agent-1".to_string(),
            session_id: "sess-1".to_string(),
            acquired_at_ms: 1_000,
            ttl_ms: 5_000,
            expires_at_ms: 6_000,
            outcome: None,
            trace_id: Some(format!("trace-{lease_id}")),
        }
    }

    fn sample_event(lease_id: &str, kind: &str) -> LeaseEventRow {
        LeaseEventRow {
            seq: None,
            lease_id: lease_id.to_string(),
            version: 2,
            kind: kind.to_string(),
            payload: format!("{{\"kind\":\"{kind}\"}}"),
            at_ms: 1_000,
        }
    }

    #[tokio::test]
    async fn row_repository_survives_restart_with_snapshot_and_events() {
        // 崩溃 / 重启恢复：快照与 append-only 事件都必须在 Actor 关闭重开后存活。
        let path = temp_path("repo-restart.sqlite3");
        {
            let actor = DatabaseActor::open(&path).await.expect("open");
            migrate(&actor, &path, false).await.expect("migrate");
            let repo = LeaseRowRepository::new(actor.clone());
            let snapshot = sample_snapshot("lease-1", "tenant-a", "acquired");
            let event = sample_event("lease-1", "acquired");
            repo.apply(&snapshot, std::slice::from_ref(&event))
                .await
                .expect("apply");
            assert_eq!(repo.load_outstanding().await.unwrap().len(), 1);
            assert_eq!(repo.event_count().await.unwrap(), 1);
            actor.shutdown().await.expect("shutdown"); // 模拟崩溃 / 重启
        }
        // 全新 Actor 重开同一文件。
        let actor = DatabaseActor::open(&path).await.expect("reopen");
        let repo = LeaseRowRepository::new(actor.clone());
        let outstanding = repo.load_outstanding().await.expect("load outstanding");
        assert_eq!(outstanding.len(), 1, "快照必须在重启后存活");
        assert_eq!(outstanding[0].lease_id, "lease-1");
        assert_eq!(outstanding[0].state, "acquired");
        assert_eq!(outstanding[0].trace_id.as_deref(), Some("trace-lease-1"));
        let events = repo.load_events("lease-1").await.expect("load events");
        assert_eq!(events.len(), 1, "append-only 事件必须在重启后存活");
        assert_eq!(events[0].kind, "acquired");
        assert!(events[0].seq.is_some(), "seq 必须由 DB 自增赋值");
        assert_eq!(events[0].payload, "{\"kind\":\"acquired\"}");
        // settle 后快照移出活跃集，事件保留。
        repo.settle("lease-1").await.expect("settle");
        assert!(repo.load_outstanding().await.unwrap().is_empty());
        assert_eq!(
            repo.load_events("lease-1").await.unwrap().len(),
            1,
            "settle 后事件日志必须保留"
        );
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn apply_is_atomic_rolls_back_on_bad_event_kind() {
        // 事件 kind 违反 CHECK 约束 → 整事务回滚：快照也不得残留（all-or-nothing）。
        let path = temp_path("repo-atomic.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        migrate(&actor, &path, false).await.expect("migrate");
        let repo = LeaseRowRepository::new(actor.clone());
        let snapshot = sample_snapshot("lease-2", "tenant-a", "acquired");
        let bad_event = LeaseEventRow {
            kind: "bogus".to_string(),
            ..sample_event("lease-2", "acquired")
        };
        let result = repo
            .apply(&snapshot, std::slice::from_ref(&bad_event))
            .await;
        assert!(result.is_err(), "非法 kind 应被 CHECK 约束拒绝");
        assert!(repo.load_outstanding().await.unwrap().is_empty());
        assert_eq!(repo.event_count().await.unwrap(), 0);
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn settle_is_idempotent() {
        // settle 对不存在的行亦成功（幂等），不报错。
        let path = temp_path("repo-settle-idem.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        migrate(&actor, &path, false).await.expect("migrate");
        let repo = LeaseRowRepository::new(actor.clone());
        repo.settle("never-existed").await.expect("settle missing");
        let snapshot = sample_snapshot("lease-3", "tenant-a", "acquired");
        repo.upsert_snapshot(&snapshot).await.expect("upsert");
        repo.settle("lease-3").await.expect("settle once");
        // 再次 settle 同一 lease 仍成功。
        repo.settle("lease-3").await.expect("settle twice");
        assert!(repo.load("lease-3").await.unwrap().is_none());
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn load_outstanding_for_tenant_is_scoped() {
        // tenant-scoped 查询只返回该租户的非终态快照（租户隔离恢复）。
        let path = temp_path("repo-tenant.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        migrate(&actor, &path, false).await.expect("migrate");
        let repo = LeaseRowRepository::new(actor.clone());
        repo.upsert_snapshot(&sample_snapshot("l-a1", "tenant-a", "acquired"))
            .await
            .expect("upsert a1");
        repo.upsert_snapshot(&sample_snapshot("l-a2", "tenant-a", "released"))
            .await
            .expect("upsert a2");
        repo.upsert_snapshot(&sample_snapshot("l-b1", "tenant-b", "acquired"))
            .await
            .expect("upsert b1");
        let a = repo
            .load_outstanding_for_tenant("tenant-a")
            .await
            .expect("load tenant-a");
        assert_eq!(a.len(), 2);
        assert!(a.iter().all(|row| row.tenant_id == "tenant-a"));
        let b = repo
            .load_outstanding_for_tenant("tenant-b")
            .await
            .expect("load tenant-b");
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].lease_id, "l-b1");
        assert_eq!(repo.load_outstanding().await.unwrap().len(), 3);
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn snapshot_row_round_trips_all_columns() {
        // 全字段往返：含 outcome / trace_id 的 released 态；终态行不进 load_outstanding。
        let path = temp_path("repo-roundtrip.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open");
        migrate(&actor, &path, false).await.expect("migrate");
        let repo = LeaseRowRepository::new(actor.clone());
        let mut snapshot = sample_snapshot("lease-rt", "tenant-a", "released");
        snapshot.version = 3;
        snapshot.outcome = Some("success".to_string());
        repo.upsert_snapshot(&snapshot).await.expect("upsert");
        let loaded = repo.load("lease-rt").await.unwrap().expect("present");
        assert_eq!(loaded, snapshot);
        // 终态（reclaimed）行不在 load_outstanding 中，但 load 仍可读。
        let mut reclaimed = snapshot.clone();
        reclaimed.state = "reclaimed".to_string();
        repo.upsert_snapshot(&reclaimed)
            .await
            .expect("upsert reclaimed");
        assert!(repo.load_outstanding().await.unwrap().is_empty());
        assert_eq!(
            repo.load("lease-rt").await.unwrap().unwrap().state,
            "reclaimed"
        );
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }
}
