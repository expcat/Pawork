//! P18-7 Session Affinity 的 SQLite 持久化行仓库与 append-only 事件日志。
//!
//! 刻意不依赖 `provider-control`（存储层不反向拉入控制面行为类型，与
//! `app-database::lease` 同一方向）：本模块只持久化扁平 [`SessionBindingRow`]
//! 与 opaque 事件 JSON；canonical `SessionBinding` 状态机、`BindingEvent` 与
//! revision/ownership_epoch CAS 语义由 `provider-control::binding` 定义，组合层
//! 在两侧间转换（P18-14 接线）。
//!
//! **本表与事件日志不含任何 secret 列**：binding 只携带 opaque 定位符与 lease
//! 引用，明文凭据由 OS Keychain 在 lease 之外解析。

use agent_domain::{AgentId, SessionId, TenantId};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::Value;
use thiserror::Error;

use app_database::{DatabaseActor, DatabaseError};

use crate::SessionStore;

/// `session_bindings` 业务表名。
pub const SESSION_BINDINGS_TABLE: &str = "session_bindings";

/// `session_binding_events` append-only 事件日志表名。
pub const SESSION_BINDING_EVENTS_TABLE: &str = "session_binding_events";

/// 绑定键的冻结 state 词表（与 `provider-control::binding::BindingState` 对齐）。
const VALID_STATES: [&str; 3] = ["bound", "rebinding", "released"];

/// 扁平 binding 快照行（状态机词表与 `provider-control::binding` 对齐）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionBindingRow {
    pub tenant_id: TenantId,
    pub session_id: SessionId,
    pub agent_id: AgentId,
    pub schema_version: u32,
    /// `bound` | `rebinding` | `released`（冻结词表，读取时 fail-closed 校验）。
    pub state: String,
    /// 乐观并发版本号（CAS 守卫之一）。
    pub revision: u64,
    /// 所有权 epoch（CAS 守卫之一）。
    pub ownership_epoch: u64,
    pub provider_id: String,
    pub model_id: String,
    pub account_id: String,
    pub credential_id: String,
    pub capability_hash: u64,
    pub policy_hash: u64,
    pub lease_id: String,
    pub bound_at_ms: u64,
    pub ttl_ms: u64,
    pub expires_at_ms: u64,
}

/// 绑定行主键 `(tenant, session, agent)`。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BindingRowKey {
    pub tenant_id: TenantId,
    pub session_id: SessionId,
    pub agent_id: AgentId,
}

impl BindingRowKey {
    /// 由 `(tenant, session, agent)` 构造。
    pub fn new(tenant_id: TenantId, session_id: SessionId, agent_id: AgentId) -> Self {
        Self {
            tenant_id,
            session_id,
            agent_id,
        }
    }

    fn label(&self) -> String {
        format!("{}/{}/{}", self.tenant_id, self.session_id, self.agent_id)
    }
}

/// append-only 事件日志中的一行（payload 为 opaque JSON，仓库不解析事件结构）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingEventRow {
    pub seq: i64,
    pub payload: Value,
}

/// binding 行仓库错误。
#[derive(Debug, Error)]
pub enum BindingRepoError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("binding row already exists for {key}")]
    AlreadyExists { key: String },
    #[error("binding row not found for {key}")]
    NotFound { key: String },
    #[error("binding row already released for {key}")]
    AlreadyReleased { key: String },
    #[error("binding row {key} is not released (state {state}); settle denied")]
    NotReleased { key: String, state: String },
    #[error(
        "binding CAS conflict for {key}: expected (revision {expected_revision}, \
         epoch {expected_epoch}), actual (revision {actual_revision}, epoch {actual_epoch})"
    )]
    Conflict {
        key: String,
        expected_revision: u64,
        expected_epoch: u64,
        actual_revision: u64,
        actual_epoch: u64,
    },
    #[error("corrupt binding row for {key}: {detail}")]
    Corrupt { key: String, detail: String },
}

/// SQLite `INTEGER` 是 i64：u64 字段经位保持 `as` 转换存取（与
/// `app-database::lease` 同一约定；相等性 / 时间比较语义不受影响）。
fn as_sql_int(value: u64) -> i64 {
    value as i64
}

fn row_key(row: &SessionBindingRow) -> BindingRowKey {
    BindingRowKey::new(
        row.tenant_id.clone(),
        row.session_id.clone(),
        row.agent_id.clone(),
    )
}

/// 冻结词表校验（fail-closed：损坏行不得静默进入上层状态机）。
fn validate_state(state: &str, key: &BindingRowKey) -> Result<(), BindingRepoError> {
    if VALID_STATES.contains(&state) {
        Ok(())
    } else {
        Err(BindingRepoError::Corrupt {
            key: key.label(),
            detail: format!("unknown binding state `{state}`"),
        })
    }
}

/// `session_bindings` 的持久化行仓库（SQLite Actor 串行化写入，天然原子 CAS）。
#[derive(Clone)]
pub struct BindingRowRepository {
    database: DatabaseActor,
}

impl BindingRowRepository {
    /// 以共享的 SQLite Actor 构造。
    pub fn new(database: DatabaseActor) -> Self {
        Self { database }
    }

    /// 初始绑定：键不存在才插入（否则 [`BindingRepoError::AlreadyExists`]），
    /// 并在同一事务追加事件（并发 double-bind 防护）。
    pub async fn insert(
        &self,
        row: &SessionBindingRow,
        events: &[Value],
    ) -> Result<(), BindingRepoError> {
        let row = row.clone();
        let events = events.to_vec();
        self.database
            .call(move |connection| {
                let transaction = connection.transaction()?;
                insert_row(&transaction, &row)?;
                append_events(&transaction, &row_key(&row), &events)?;
                transaction.commit()?;
                Ok(())
            })
            .await?
    }

    /// 乐观并发 CAS：当前行的 `(revision, ownership_epoch)` 完全匹配才覆盖，
    /// 并在同一事务追加事件。`released` 行同样可被守卫匹配的 CAS 覆盖——
    /// `Released → Bound` 重绑经此延续 revision / epoch，事件日志保持连续；
    /// 守卫不匹配的 `released` 行返回 [`BindingRepoError::AlreadyReleased`]。
    pub async fn compare_and_update(
        &self,
        key: &BindingRowKey,
        expected_revision: u64,
        expected_epoch: u64,
        row: &SessionBindingRow,
        events: &[Value],
    ) -> Result<(), BindingRepoError> {
        let key = key.clone();
        let row = row.clone();
        let events = events.to_vec();
        self.database
            .call(move |connection| {
                let transaction = connection.transaction()?;
                let updated = transaction.execute(
                    "UPDATE session_bindings SET \
                     schema_version=?4, state=?5, revision=?6, ownership_epoch=?7, \
                     provider_id=?8, model_id=?9, account_id=?10, credential_id=?11, \
                     capability_hash=?12, policy_hash=?13, lease_id=?14, bound_at_ms=?15, \
                     ttl_ms=?16, expires_at_ms=?17 \
                     WHERE tenant_id=?1 AND session_id=?2 AND agent_id=?3 \
                       AND revision=?18 AND ownership_epoch=?19",
                    params![
                        row.tenant_id.as_str(),
                        row.session_id.as_str(),
                        row.agent_id.as_str(),
                        row.schema_version,
                        row.state,
                        as_sql_int(row.revision),
                        as_sql_int(row.ownership_epoch),
                        row.provider_id,
                        row.model_id,
                        row.account_id,
                        row.credential_id,
                        as_sql_int(row.capability_hash),
                        as_sql_int(row.policy_hash),
                        row.lease_id,
                        as_sql_int(row.bound_at_ms),
                        as_sql_int(row.ttl_ms),
                        as_sql_int(row.expires_at_ms),
                        as_sql_int(expected_revision),
                        as_sql_int(expected_epoch),
                    ],
                )?;
                if updated == 0 {
                    return Err(cas_failure(
                        &transaction,
                        &key,
                        expected_revision,
                        expected_epoch,
                    )?);
                }
                append_events(&transaction, &key, &events)?;
                transaction.commit()?;
                Ok(())
            })
            .await?
    }

    /// 重放 / 对账修复。快照可覆盖；事件按 `version` 幂等追加：已存在且内容
    /// 相同的事件跳过，同版本不同内容或版本跳跃 fail-closed，保证日志可按严格
    /// `+1` revision 重放。
    pub async fn upsert(
        &self,
        row: &SessionBindingRow,
        events: &[Value],
    ) -> Result<(), BindingRepoError> {
        let row = row.clone();
        let events = events.to_vec();
        self.database
            .call(move |connection| {
                let transaction = connection.transaction()?;
                transaction.execute(
                    "INSERT INTO session_bindings (\
                     tenant_id, session_id, agent_id, schema_version, state, revision, \
                     ownership_epoch, provider_id, model_id, account_id, credential_id, \
                     capability_hash, policy_hash, lease_id, bound_at_ms, ttl_ms, expires_at_ms\
                     ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17) \
                     ON CONFLICT(tenant_id, session_id, agent_id) DO UPDATE SET \
                     schema_version=excluded.schema_version, state=excluded.state, \
                     revision=excluded.revision, ownership_epoch=excluded.ownership_epoch, \
                     provider_id=excluded.provider_id, model_id=excluded.model_id, \
                     account_id=excluded.account_id, credential_id=excluded.credential_id, \
                     capability_hash=excluded.capability_hash, \
                     policy_hash=excluded.policy_hash, lease_id=excluded.lease_id, \
                     bound_at_ms=excluded.bound_at_ms, ttl_ms=excluded.ttl_ms, \
                     expires_at_ms=excluded.expires_at_ms",
                    params![
                        row.tenant_id.as_str(),
                        row.session_id.as_str(),
                        row.agent_id.as_str(),
                        row.schema_version,
                        row.state,
                        as_sql_int(row.revision),
                        as_sql_int(row.ownership_epoch),
                        row.provider_id,
                        row.model_id,
                        row.account_id,
                        row.credential_id,
                        as_sql_int(row.capability_hash),
                        as_sql_int(row.policy_hash),
                        row.lease_id,
                        as_sql_int(row.bound_at_ms),
                        as_sql_int(row.ttl_ms),
                        as_sql_int(row.expires_at_ms),
                    ],
                )?;
                append_replay_events(&transaction, &row_key(&row), &events)?;
                transaction.commit()?;
                Ok(())
            })
            .await?
    }
}

/// `INSERT OR IGNORE`：已存在则报 `AlreadyExists`（插入侧并发 double-bind 防护）。
fn insert_row(
    transaction: &Transaction<'_>,
    row: &SessionBindingRow,
) -> Result<(), BindingRepoError> {
    let inserted = transaction.execute(
        "INSERT OR IGNORE INTO session_bindings (\
         tenant_id, session_id, agent_id, schema_version, state, revision, \
         ownership_epoch, provider_id, model_id, account_id, credential_id, \
         capability_hash, policy_hash, lease_id, bound_at_ms, ttl_ms, expires_at_ms\
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
        params![
            row.tenant_id.as_str(),
            row.session_id.as_str(),
            row.agent_id.as_str(),
            row.schema_version,
            row.state,
            as_sql_int(row.revision),
            as_sql_int(row.ownership_epoch),
            row.provider_id,
            row.model_id,
            row.account_id,
            row.credential_id,
            as_sql_int(row.capability_hash),
            as_sql_int(row.policy_hash),
            row.lease_id,
            as_sql_int(row.bound_at_ms),
            as_sql_int(row.ttl_ms),
            as_sql_int(row.expires_at_ms),
        ],
    )?;
    if inserted == 0 {
        return Err(BindingRepoError::AlreadyExists {
            key: row_key(row).label(),
        });
    }
    Ok(())
}

/// CAS 未命中时区分 NotFound / AlreadyReleased / Conflict（供准确错误与重试）。
fn cas_failure(
    transaction: &Transaction<'_>,
    key: &BindingRowKey,
    expected_revision: u64,
    expected_epoch: u64,
) -> Result<BindingRepoError, BindingRepoError> {
    let current: Option<(String, i64, i64)> = transaction
        .query_row(
            "SELECT state, revision, ownership_epoch FROM session_bindings \
             WHERE tenant_id=?1 AND session_id=?2 AND agent_id=?3",
            params![
                key.tenant_id.as_str(),
                key.session_id.as_str(),
                key.agent_id.as_str()
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    Ok(match current {
        None => BindingRepoError::NotFound { key: key.label() },
        Some((state, _, _)) if state == "released" => {
            BindingRepoError::AlreadyReleased { key: key.label() }
        }
        Some((_, revision, epoch)) => BindingRepoError::Conflict {
            key: key.label(),
            expected_revision,
            expected_epoch,
            actual_revision: revision as u64,
            actual_epoch: epoch as u64,
        },
    })
}

/// 在事务内按序追加 opaque 事件 JSON（`seq` 由 SQLite 自增，重放按 `seq` 排序）。
fn append_events(
    transaction: &Transaction<'_>,
    key: &BindingRowKey,
    events: &[Value],
) -> Result<(), BindingRepoError> {
    for event in events {
        transaction.execute(
            "INSERT INTO session_binding_events(\
             tenant_id, session_id, agent_id, event_json, appended_at_ms) \
             VALUES (?1, ?2, ?3, ?4, CAST(strftime('%s','now') AS INTEGER) * 1000)",
            params![
                key.tenant_id.as_str(),
                key.session_id.as_str(),
                key.agent_id.as_str(),
                serde_json::to_string(event)?,
            ],
        )?;
    }
    Ok(())
}

/// 对修复式重放做版本级幂等。普通状态转换仍走 [`append_events`]，由状态机/CAS
/// 保证只提交一次；这里额外处理宿主在崩溃恢复时重复提交同一批事件的场景。
fn append_replay_events(
    transaction: &Transaction<'_>,
    key: &BindingRowKey,
    events: &[Value],
) -> Result<(), BindingRepoError> {
    let mut statement = transaction.prepare(
        "SELECT event_json FROM session_binding_events \
         WHERE tenant_id=?1 AND session_id=?2 AND agent_id=?3 ORDER BY seq",
    )?;
    let rows = statement.query_map(
        params![
            key.tenant_id.as_str(),
            key.session_id.as_str(),
            key.agent_id.as_str()
        ],
        |row| row.get::<_, String>(0),
    )?;
    let mut existing = std::collections::BTreeMap::<u64, Value>::new();
    for payload in rows {
        let payload: Value = serde_json::from_str(&payload?)?;
        let version = event_version(&payload, key)?;
        match existing.get(&version) {
            Some(previous) if previous != &payload => {
                return Err(BindingRepoError::Corrupt {
                    key: key.label(),
                    detail: format!("conflicting persisted events at revision {version}"),
                });
            }
            Some(_) => {}
            None => {
                existing.insert(version, payload);
            }
        }
    }
    drop(statement);

    let mut high_watermark = existing.keys().next_back().copied().unwrap_or(0);
    let mut pending = Vec::new();
    for event in events {
        let version = event_version(event, key)?;
        if let Some(previous) = existing.get(&version) {
            if previous != event {
                return Err(BindingRepoError::Corrupt {
                    key: key.label(),
                    detail: format!("conflicting replay event at revision {version}"),
                });
            }
            continue;
        }
        let expected = high_watermark.saturating_add(1);
        if version != expected {
            return Err(BindingRepoError::Corrupt {
                key: key.label(),
                detail: format!(
                    "replay event revision {version} is not contiguous after {high_watermark}"
                ),
            });
        }
        high_watermark = version;
        existing.insert(version, event.clone());
        pending.push(event.clone());
    }
    append_events(transaction, key, &pending)
}

fn event_version(event: &Value, key: &BindingRowKey) -> Result<u64, BindingRepoError> {
    event
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| BindingRepoError::Corrupt {
            key: key.label(),
            detail: "binding event is missing unsigned integer `version`".to_string(),
        })
}

/// 从行中读取快照（state 词表 fail-closed，u64 经位保持转换还原）。
fn map_row(row: &rusqlite::Row<'_>, key: &BindingRowKey) -> rusqlite::Result<SessionBindingRow> {
    Ok(SessionBindingRow {
        tenant_id: key.tenant_id.clone(),
        session_id: key.session_id.clone(),
        agent_id: key.agent_id.clone(),
        schema_version: row.get::<_, i64>(0)? as u32,
        state: row.get::<_, String>(1)?,
        revision: row.get::<_, i64>(2)? as u64,
        ownership_epoch: row.get::<_, i64>(3)? as u64,
        provider_id: row.get::<_, String>(4)?,
        model_id: row.get::<_, String>(5)?,
        account_id: row.get::<_, String>(6)?,
        credential_id: row.get::<_, String>(7)?,
        capability_hash: row.get::<_, i64>(8)? as u64,
        policy_hash: row.get::<_, i64>(9)? as u64,
        lease_id: row.get::<_, String>(10)?,
        bound_at_ms: row.get::<_, i64>(11)? as u64,
        ttl_ms: row.get::<_, i64>(12)? as u64,
        expires_at_ms: row.get::<_, i64>(13)? as u64,
    })
}

impl BindingRowRepository {
    /// 读取当前快照（含 released；不存在返回 `None`）。
    pub async fn load(
        &self,
        key: &BindingRowKey,
    ) -> Result<Option<SessionBindingRow>, BindingRepoError> {
        let key = key.clone();
        self.database
            .call(move |connection| load_row(connection, &key))
            .await?
    }

    /// 所有非 released 快照（恢复扫描 / 孤儿 rebinding 收敛用）。
    pub async fn load_outstanding(&self) -> Result<Vec<SessionBindingRow>, BindingRepoError> {
        self.database
            .call(|connection| {
                let mut statement = connection.prepare(
                    "SELECT schema_version, state, revision, ownership_epoch, provider_id, \
                     model_id, account_id, credential_id, capability_hash, policy_hash, \
                     lease_id, bound_at_ms, ttl_ms, expires_at_ms, tenant_id, session_id, \
                     agent_id FROM session_bindings WHERE state != 'released'",
                )?;
                let rows = statement.query_map([], |row| {
                    let key = BindingRowKey::new(
                        TenantId::new(row.get::<_, String>(14)?),
                        SessionId::new(row.get::<_, String>(15)?),
                        AgentId::new(row.get::<_, String>(16)?),
                    );
                    map_row(row, &key)
                })?;
                let mut result = Vec::new();
                for row in rows {
                    result.push(row?);
                }
                Ok(result)
            })
            .await?
    }

    /// 按键读取 append-only 事件日志（按 `seq` 升序，供 crash / 重启重放）。
    pub async fn events(
        &self,
        key: &BindingRowKey,
    ) -> Result<Vec<BindingEventRow>, BindingRepoError> {
        let key = key.clone();
        self.database
            .call(move |connection| {
                let mut statement = connection.prepare(
                    "SELECT seq, event_json FROM session_binding_events \
                     WHERE tenant_id=?1 AND session_id=?2 AND agent_id=?3 ORDER BY seq",
                )?;
                let rows = statement.query_map(
                    params![
                        key.tenant_id.as_str(),
                        key.session_id.as_str(),
                        key.agent_id.as_str()
                    ],
                    |row| {
                        let seq: i64 = row.get(0)?;
                        let payload: String = row.get(1)?;
                        Ok((seq, payload))
                    },
                )?;
                let mut result = Vec::new();
                for row in rows {
                    let (seq, payload) = row?;
                    result.push(BindingEventRow {
                        seq,
                        payload: serde_json::from_str(&payload).map_err(|error| {
                            BindingRepoError::Corrupt {
                                key: key.label(),
                                detail: format!("invalid event JSON at seq {seq}: {error}"),
                            }
                        })?,
                    });
                }
                Ok(result)
            })
            .await?
    }

    /// released 行移出活跃集合（GC）；事件日志保留（审计 / 重放）。
    ///
    /// **只允许 `released`**：行存在但非 released 返回
    /// [`BindingRepoError::NotReleased`]（fail-closed，不误删在用行）；
    /// 行不存在视为已 GC（幂等 `Ok`）。
    pub async fn settle(&self, key: &BindingRowKey) -> Result<(), BindingRepoError> {
        let key = key.clone();
        self.database
            .call(move |connection| {
                let state: Option<String> = connection
                    .query_row(
                        "SELECT state FROM session_bindings \
                         WHERE tenant_id=?1 AND session_id=?2 AND agent_id=?3",
                        params![
                            key.tenant_id.as_str(),
                            key.session_id.as_str(),
                            key.agent_id.as_str()
                        ],
                        |row| row.get(0),
                    )
                    .optional()?;
                match state {
                    None => Ok(()),
                    Some(state) if state == "released" => {
                        connection.execute(
                            "DELETE FROM session_bindings \
                             WHERE tenant_id=?1 AND session_id=?2 AND agent_id=?3",
                            params![
                                key.tenant_id.as_str(),
                                key.session_id.as_str(),
                                key.agent_id.as_str()
                            ],
                        )?;
                        Ok(())
                    }
                    Some(state) => Err(BindingRepoError::NotReleased {
                        key: key.label(),
                        state,
                    }),
                }
            })
            .await?
    }

    /// 该键的世代高水位 `(revision, ownership_epoch)`：从保留的 append-only
    /// 事件日志读出最近一次事件的 `version` 与最近一条 `bound` 事件的
    /// `ownership_epoch`。settle（GC）后行已删除而事件仍保留，此高水位是
    /// GC 后再 bind 严格延续 generation 的唯一事实源；无任何事件返回 `None`。
    pub async fn continuation(
        &self,
        key: &BindingRowKey,
    ) -> Result<Option<(u64, u64)>, BindingRepoError> {
        let key = key.clone();
        self.database
            .call(move |connection| {
                let mut statement = connection.prepare(
                    "SELECT event_json FROM session_binding_events \
                     WHERE tenant_id=?1 AND session_id=?2 AND agent_id=?3 ORDER BY seq",
                )?;
                let rows = statement.query_map(
                    params![
                        key.tenant_id.as_str(),
                        key.session_id.as_str(),
                        key.agent_id.as_str()
                    ],
                    |row| row.get::<_, String>(0),
                )?;
                let mut revision = None;
                let mut epoch = None;
                for payload in rows {
                    let payload: Value = serde_json::from_str(&payload?)?;
                    if let Some(version) = payload.get("version").and_then(Value::as_u64) {
                        revision = Some(version);
                    }
                    if payload.get("kind").and_then(Value::as_str) == Some("bound") {
                        if let Some(ownership_epoch) =
                            payload.get("ownership_epoch").and_then(Value::as_u64)
                        {
                            epoch = Some(ownership_epoch);
                        }
                    }
                }
                Ok(revision.zip(epoch))
            })
            .await?
    }
}

fn load_row(
    connection: &Connection,
    key: &BindingRowKey,
) -> Result<Option<SessionBindingRow>, BindingRepoError> {
    let row = connection
        .query_row(
            "SELECT schema_version, state, revision, ownership_epoch, provider_id, \
             model_id, account_id, credential_id, capability_hash, policy_hash, lease_id, \
             bound_at_ms, ttl_ms, expires_at_ms FROM session_bindings \
             WHERE tenant_id=?1 AND session_id=?2 AND agent_id=?3",
            params![
                key.tenant_id.as_str(),
                key.session_id.as_str(),
                key.agent_id.as_str()
            ],
            |row| map_row(row, key),
        )
        .optional()?;
    match row {
        None => Ok(None),
        Some(row) => {
            validate_state(&row.state, key)?;
            Ok(Some(row))
        }
    }
}

impl SessionStore {
    /// P18-7 binding 投影行仓库（flat rows + CAS + append-only 事件日志）。
    pub fn binding_repository(&self) -> BindingRowRepository {
        BindingRowRepository::new(self.database().clone())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc,
        },
    };

    use serde_json::json;

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_path(name: &str) -> PathBuf {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pawork-session-binding-{}-{unique}-{name}",
            std::process::id()
        ))
    }

    fn key(tenant: &str) -> BindingRowKey {
        BindingRowKey::new(
            TenantId::new(tenant),
            SessionId::new("session-1"),
            AgentId::new("agent-1"),
        )
    }

    fn row(tenant: &str, revision: u64, epoch: u64, lease: &str) -> SessionBindingRow {
        SessionBindingRow {
            tenant_id: TenantId::new(tenant),
            session_id: SessionId::new("session-1"),
            agent_id: AgentId::new("agent-1"),
            schema_version: 2,
            state: "bound".to_string(),
            revision,
            ownership_epoch: epoch,
            provider_id: "prov-1".to_string(),
            model_id: "model-1".to_string(),
            account_id: "acct-1".to_string(),
            credential_id: "cred-1".to_string(),
            capability_hash: 7,
            policy_hash: 11,
            lease_id: lease.to_string(),
            bound_at_ms: 1_000,
            ttl_ms: 60_000,
            expires_at_ms: 61_000,
        }
    }

    fn bound_event(version: u64, lease: &str) -> Value {
        json!({
            "kind": "bound",
            "version": version,
            "ownership_epoch": 0,
            "lease_id": lease,
        })
    }

    #[tokio::test]
    async fn insert_load_round_trip_and_cas_conflict() {
        let path = temp_path("cas.sqlite3");
        let (store, _) = SessionStore::open(&path).await.expect("open store");
        let repo = store.binding_repository();

        repo.insert(
            &row("tenant-a", 1, 0, "lease-1"),
            &[bound_event(1, "lease-1")],
        )
        .await
        .expect("insert");
        assert_eq!(
            repo.load(&key("tenant-a")).await.expect("load"),
            Some(row("tenant-a", 1, 0, "lease-1"))
        );

        // 同键重复插入：AlreadyExists（并发 double-bind 防护）。
        assert!(matches!(
            repo.insert(&row("tenant-a", 1, 0, "lease-1"), &[]).await,
            Err(BindingRepoError::AlreadyExists { .. })
        ));

        // CAS：过期 revision / epoch 均 Conflict，且行未被覆盖。
        assert!(matches!(
            repo.compare_and_update(
                &key("tenant-a"),
                9,
                0,
                &row("tenant-a", 10, 0, "lease-2"),
                &[]
            )
            .await,
            Err(BindingRepoError::Conflict {
                expected_revision: 9,
                actual_revision: 1,
                ..
            })
        ));
        assert!(matches!(
            repo.compare_and_update(
                &key("tenant-a"),
                1,
                5,
                &row("tenant-a", 2, 5, "lease-2"),
                &[]
            )
            .await,
            Err(BindingRepoError::Conflict {
                expected_epoch: 5,
                actual_epoch: 0,
                ..
            })
        ));

        // 匹配守卫的 CAS 成功且原子覆盖。
        repo.compare_and_update(
            &key("tenant-a"),
            1,
            0,
            &row("tenant-a", 2, 0, "lease-2"),
            &[bound_event(2, "lease-2")],
        )
        .await
        .expect("cas");
        assert_eq!(
            repo.load(&key("tenant-a")).await.expect("load"),
            Some(row("tenant-a", 2, 0, "lease-2"))
        );
        // 未知键 CAS：NotFound。
        assert!(matches!(
            repo.compare_and_update(
                &key("tenant-b"),
                1,
                0,
                &row("tenant-b", 1, 0, "lease-1"),
                &[]
            )
            .await,
            Err(BindingRepoError::NotFound { .. })
        ));

        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn concurrent_insert_exactly_one_wins() {
        let path = temp_path("double-bind.sqlite3");
        let (store, _) = SessionStore::open(&path).await.expect("open store");
        let repo = Arc::new(store.binding_repository());
        let mut tasks = Vec::new();
        for _ in 0..2 {
            let repo = repo.clone();
            tasks.push(tokio::spawn(async move {
                repo.insert(
                    &row("tenant-a", 1, 0, "lease-1"),
                    &[bound_event(1, "lease-1")],
                )
                .await
            }));
        }
        let mut results = Vec::new();
        for task in tasks {
            results.push(task.await.expect("task"));
        }
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, Err(BindingRepoError::AlreadyExists { .. })))
                .count(),
            1
        );
        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn cross_tenant_bindings_are_fully_isolated() {
        let path = temp_path("tenant.sqlite3");
        let (store, _) = SessionStore::open(&path).await.expect("open store");
        let repo = store.binding_repository();
        repo.insert(
            &row("tenant-a", 1, 0, "lease-a"),
            &[bound_event(1, "lease-a")],
        )
        .await
        .expect("insert a");
        repo.insert(
            &row("tenant-b", 1, 0, "lease-b"),
            &[bound_event(1, "lease-b")],
        )
        .await
        .expect("insert b");

        // 同名 session/agent 在两个租户下互不可见（P18-7 验收：跨租户禁止复用）。
        assert_eq!(
            repo.load(&key("tenant-b")).await.expect("load"),
            Some(row("tenant-b", 1, 0, "lease-b"))
        );
        assert_eq!(repo.load_outstanding().await.expect("outstanding").len(), 2);

        // Tenant B 的 CAS / settle / 事件查询不得影响 Tenant A。
        repo.compare_and_update(
            &key("tenant-b"),
            1,
            0,
            &row("tenant-b", 2, 1, "lease-b2"),
            &[bound_event(2, "lease-b2")],
        )
        .await
        .expect("cas b");
        assert_eq!(
            repo.load(&key("tenant-a")).await.expect("load a"),
            Some(row("tenant-a", 1, 0, "lease-a"))
        );
        let events_a = repo.events(&key("tenant-a")).await.expect("events a");
        assert_eq!(events_a.len(), 1);
        assert_eq!(events_a[0].payload, bound_event(1, "lease-a"));

        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn released_rows_are_excluded_and_stale_guards_rejected() {
        let path = temp_path("released.sqlite3");
        let (store, _) = SessionStore::open(&path).await.expect("open store");
        let repo = store.binding_repository();
        let mut released = row("tenant-a", 3, 0, "lease-1");
        released.state = "released".to_string();
        repo.upsert(&released, &[bound_event(1, "lease-1")])
            .await
            .expect("upsert released");

        assert!(repo
            .load_outstanding()
            .await
            .expect("outstanding")
            .is_empty());
        // 守卫不匹配的 released 行仍拒绝覆盖（必须重读当前 revision / epoch）。
        assert!(matches!(
            repo.compare_and_update(
                &key("tenant-a"),
                2,
                0,
                &row("tenant-a", 4, 0, "lease-2"),
                &[]
            )
            .await,
            Err(BindingRepoError::AlreadyReleased { .. })
        ));
        // settle 只删 released 行；事件日志保留。
        repo.settle(&key("tenant-a")).await.expect("settle");
        assert_eq!(repo.load(&key("tenant-a")).await.expect("load"), None);
        assert_eq!(
            repo.events(&key("tenant-a")).await.expect("events").len(),
            1
        );

        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn released_to_bound_cas_continues_revision_epoch_and_events() {
        let path = temp_path("released-to-bound.sqlite3");
        let (store, _) = SessionStore::open(&path).await.expect("open store");
        let repo = store.binding_repository();
        repo.insert(
            &row("tenant-a", 1, 0, "lease-1"),
            &[bound_event(1, "lease-1")],
        )
        .await
        .expect("insert");

        // Bound → Released（canonical release：revision +1，epoch 不变）。
        let mut released = row("tenant-a", 2, 0, "lease-1");
        released.state = "released".to_string();
        repo.compare_and_update(
            &key("tenant-a"),
            1,
            0,
            &released,
            &[json!({"kind": "released", "version": 2})],
        )
        .await
        .expect("release cas");
        assert!(repo
            .load_outstanding()
            .await
            .expect("outstanding")
            .is_empty());

        // Released → Bound（重绑）：守卫匹配的 CAS 覆盖 released 行，revision /
        // epoch 各 +1 延续 generation，事件按 seq 严格连续追加。
        repo.compare_and_update(
            &key("tenant-a"),
            2,
            0,
            &row("tenant-a", 3, 1, "lease-2"),
            &[json!({
                "kind": "bound",
                "version": 3,
                "ownership_epoch": 1,
                "lease_id": "lease-2",
            })],
        )
        .await
        .expect("released to bound cas");
        assert_eq!(
            repo.load(&key("tenant-a")).await.expect("load"),
            Some(row("tenant-a", 3, 1, "lease-2"))
        );
        assert_eq!(repo.load_outstanding().await.expect("outstanding").len(), 1);

        let events = repo.events(&key("tenant-a")).await.expect("events");
        let versions: Vec<u64> = events
            .iter()
            .map(|event| event.payload["version"].as_u64().expect("version"))
            .collect();
        assert_eq!(versions, vec![1, 2, 3], "Released → Bound 事件版本严格连续");

        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn events_replay_across_store_reopen() {
        let path = temp_path("replay.sqlite3");
        {
            let (store, _) = SessionStore::open(&path).await.expect("open store");
            let repo = store.binding_repository();
            repo.insert(
                &row("tenant-a", 1, 0, "lease-1"),
                &[
                    json!({"kind": "bound", "version": 1, "lease_id": "lease-1"}),
                    json!({"kind": "rebinding_started", "version": 2, "reason": "ttl_expired"}),
                    json!({"kind": "bound", "version": 3, "ownership_epoch": 1, "lease_id": "lease-2"}),
                ],
            )
            .await
            .expect("insert with events");
            store.shutdown().await.expect("shutdown");
        }
        // 崩溃 / 重启：重开 store，快照与事件日志均可恢复且有序。
        let (store, report) = SessionStore::open(&path).await.expect("reopen store");
        assert!(report.applied_versions.is_empty(), "第二次打开不重放迁移");
        let repo = store.binding_repository();
        assert_eq!(
            repo.load(&key("tenant-a")).await.expect("load"),
            Some(row("tenant-a", 1, 0, "lease-1")),
            "快照行跨重启保留"
        );
        let events = repo.events(&key("tenant-a")).await.expect("events");
        let versions: Vec<u64> = events
            .iter()
            .map(|event| event.payload["version"].as_u64().expect("version"))
            .collect();
        assert_eq!(versions, vec![1, 2, 3], "事件按 seq 升序重放");
        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn upsert_replay_is_idempotent_and_keeps_events() {
        let path = temp_path("upsert.sqlite3");
        let (store, _) = SessionStore::open(&path).await.expect("open store");
        let repo = store.binding_repository();
        repo.upsert(
            &row("tenant-a", 1, 0, "lease-1"),
            &[bound_event(1, "lease-1")],
        )
        .await
        .expect("upsert");
        repo.upsert(
            &row("tenant-a", 1, 0, "lease-1"),
            &[bound_event(1, "lease-1")],
        )
        .await
        .expect("idempotent upsert");
        assert_eq!(repo.load_outstanding().await.expect("outstanding").len(), 1);
        assert_eq!(
            repo.events(&key("tenant-a")).await.expect("events").len(),
            1,
            "identical repair replay must not duplicate the canonical revision"
        );
        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn settle_only_allows_released_and_missing_row_is_idempotent() {
        let path = temp_path("settle-guard.sqlite3");
        let (store, _) = SessionStore::open(&path).await.expect("open store");
        let repo = store.binding_repository();
        repo.insert(
            &row("tenant-a", 1, 0, "lease-1"),
            &[bound_event(1, "lease-1")],
        )
        .await
        .expect("insert");

        // 在用（bound）行拒绝 settle：fail-closed，行保持原样。
        assert!(matches!(
            repo.settle(&key("tenant-a")).await,
            Err(BindingRepoError::NotReleased { state, .. }) if state == "bound"
        ));
        assert_eq!(
            repo.load(&key("tenant-a")).await.expect("load"),
            Some(row("tenant-a", 1, 0, "lease-1"))
        );

        // Released 行才允许 settle；事件日志保留；行已 GC 后重复 settle 幂等。
        let mut released = row("tenant-a", 2, 0, "lease-1");
        released.state = "released".to_string();
        repo.compare_and_update(
            &key("tenant-a"),
            1,
            0,
            &released,
            &[json!({"kind": "released", "version": 2})],
        )
        .await
        .expect("release cas");
        repo.settle(&key("tenant-a")).await.expect("settle");
        assert_eq!(repo.load(&key("tenant-a")).await.expect("load"), None);
        assert_eq!(
            repo.events(&key("tenant-a")).await.expect("events").len(),
            2
        );
        repo.settle(&key("tenant-a"))
            .await
            .expect("idempotent settle");

        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn gc_then_bind_continues_generation_from_event_log() {
        let path = temp_path("gc-continuation.sqlite3");
        let (store, _) = SessionStore::open(&path).await.expect("open store");
        let repo = store.binding_repository();
        // 历史：bound v1(epoch 0) → rebind bound v2(epoch 1) → release v3。
        repo.insert(
            &row("tenant-a", 1, 0, "lease-1"),
            &[bound_event(1, "lease-1")],
        )
        .await
        .expect("insert");
        repo.compare_and_update(
            &key("tenant-a"),
            1,
            0,
            &row("tenant-a", 2, 1, "lease-2"),
            &[json!({
                "kind": "bound",
                "version": 2,
                "ownership_epoch": 1,
                "lease_id": "lease-2",
            })],
        )
        .await
        .expect("rebind cas");
        let mut released = row("tenant-a", 3, 1, "lease-2");
        released.state = "released".to_string();
        repo.compare_and_update(
            &key("tenant-a"),
            2,
            1,
            &released,
            &[json!({"kind": "released", "version": 3})],
        )
        .await
        .expect("release cas");

        // settle（GC）：行删除，事件日志保留。
        repo.settle(&key("tenant-a")).await.expect("settle");
        assert_eq!(repo.load(&key("tenant-a")).await.expect("load"), None);

        // GC 后的世代高水位：最近事件 version=3、最近 bound 事件 epoch=1。
        assert_eq!(
            repo.continuation(&key("tenant-a"))
                .await
                .expect("continuation"),
            Some((3, 1))
        );
        // 无历史键无高水位。
        assert_eq!(
            repo.continuation(&key("tenant-b"))
                .await
                .expect("continuation"),
            None
        );

        // 高水位续绑：v4 / epoch 2，事件日志严格连续，绝不重置 v1 / 复用 epoch。
        repo.insert(
            &row("tenant-a", 4, 2, "lease-3"),
            &[json!({
                "kind": "bound",
                "version": 4,
                "ownership_epoch": 2,
                "lease_id": "lease-3",
            })],
        )
        .await
        .expect("bind after gc");
        assert_eq!(
            repo.load(&key("tenant-a")).await.expect("load"),
            Some(row("tenant-a", 4, 2, "lease-3"))
        );
        let events = repo.events(&key("tenant-a")).await.expect("events");
        let versions: Vec<u64> = events
            .iter()
            .map(|event| event.payload["version"].as_u64().expect("version"))
            .collect();
        assert_eq!(versions, vec![1, 2, 3, 4], "GC 后事件版本严格连续");

        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }
}
