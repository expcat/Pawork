use agent_domain::{ConnectionId, SessionId, Timestamp};
use async_trait::async_trait;
use client_adapter_api::{
    AdapterError, CapabilitySnapshot, ClientProtocol, ClientSessionId, ClientSessionRecord,
    ClientSessionState, RegistryWriteOutcome, SessionRegistryStore,
};
use rusqlite::params;

use crate::SessionStore;

#[derive(Clone)]
pub struct SqliteClientSessionRegistryStore {
    store: SessionStore,
}

impl SqliteClientSessionRegistryStore {
    pub fn new(store: SessionStore) -> Self {
        Self { store }
    }

    pub fn session_store(&self) -> &SessionStore {
        &self.store
    }
}

#[async_trait]
impl SessionRegistryStore for SqliteClientSessionRegistryStore {
    async fn load_all(&self) -> Result<Vec<ClientSessionRecord>, AdapterError> {
        self.store
            .database()
            .call(
                |connection| -> Result<Vec<ClientSessionRecord>, AdapterError> {
                    let mut statement = connection
                        .prepare(
                            "SELECT client_session_id, schema_version, protocol, core_session_id, \
                         connection_id, ownership_epoch, revision, state, capability_json, \
                         updated_at_ms FROM client_adapter_sessions ORDER BY client_session_id",
                        )
                        .map_err(database_error)?;
                    let rows = statement
                        .query_map([], |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, i64>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, String>(3)?,
                                row.get::<_, String>(4)?,
                                row.get::<_, i64>(5)?,
                                row.get::<_, i64>(6)?,
                                row.get::<_, String>(7)?,
                                row.get::<_, String>(8)?,
                                row.get::<_, i64>(9)?,
                            ))
                        })
                        .map_err(database_error)?;
                    rows.map(|row| decode_record(row.map_err(database_error)?))
                        .collect()
                },
            )
            .await
            .map_err(|error| AdapterError::HostUnavailable(error.to_string()))?
    }

    async fn insert(
        &self,
        record: &ClientSessionRecord,
    ) -> Result<RegistryWriteOutcome, AdapterError> {
        let record = record.clone();
        self.store
            .database()
            .call(
                move |connection| -> Result<RegistryWriteOutcome, AdapterError> {
                    let schema_version =
                        stored_i64(u64::from(record.schema_version), "schema_version")?;
                    let ownership_epoch = stored_i64(record.ownership_epoch, "ownership_epoch")?;
                    let revision = stored_i64(record.revision, "revision")?;
                    let updated_at_ms =
                        stored_i64(record.updated_at.as_unix_millis(), "updated_at")?;
                    let state = serde_json::to_string(&record.state)
                        .map_err(|error| AdapterError::HostUnavailable(error.to_string()))?;
                    let capability_json = serde_json::to_string(&record.capabilities)
                        .map_err(|error| AdapterError::HostUnavailable(error.to_string()))?;
                    let changed = connection
                        .execute(
                            "INSERT INTO client_adapter_sessions \
                         (client_session_id, schema_version, protocol, core_session_id, \
                          connection_id, ownership_epoch, revision, state, capability_json, \
                          updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
                         ON CONFLICT(client_session_id) DO NOTHING",
                            params![
                                record.client_session_id.0,
                                schema_version,
                                record.protocol.0,
                                record.core_session_id.as_str(),
                                record.connection_id.as_str(),
                                ownership_epoch,
                                revision,
                                state,
                                capability_json,
                                updated_at_ms,
                            ],
                        )
                        .map_err(database_error)?;
                    if changed == 1 {
                        Ok(RegistryWriteOutcome::Applied)
                    } else {
                        load_one(connection, &record.client_session_id)
                            .map(|record| RegistryWriteOutcome::Conflict(Box::new(Some(record))))
                    }
                },
            )
            .await
            .map_err(|error| AdapterError::HostUnavailable(error.to_string()))?
    }

    async fn compare_and_swap(
        &self,
        expected_epoch: u64,
        expected_revision: u64,
        record: &ClientSessionRecord,
    ) -> Result<RegistryWriteOutcome, AdapterError> {
        let record = record.clone();
        self.store
            .database()
            .call(
                move |connection| -> Result<RegistryWriteOutcome, AdapterError> {
                    let schema_version =
                        stored_i64(u64::from(record.schema_version), "schema_version")?;
                    let ownership_epoch = stored_i64(record.ownership_epoch, "ownership_epoch")?;
                    let revision = stored_i64(record.revision, "revision")?;
                    let expected_epoch = stored_i64(expected_epoch, "expected_epoch")?;
                    let expected_revision = stored_i64(expected_revision, "expected_revision")?;
                    let updated_at_ms =
                        stored_i64(record.updated_at.as_unix_millis(), "updated_at")?;
                    let state = serde_json::to_string(&record.state)
                        .map_err(|error| AdapterError::HostUnavailable(error.to_string()))?;
                    let capability_json = serde_json::to_string(&record.capabilities)
                        .map_err(|error| AdapterError::HostUnavailable(error.to_string()))?;
                    let changed = connection
                        .execute(
                            "UPDATE client_adapter_sessions SET schema_version=?2, protocol=?3, \
                         core_session_id=?4, connection_id=?5, ownership_epoch=?6, revision=?7, \
                         state=?8, capability_json=?9, updated_at_ms=?10 \
                         WHERE client_session_id=?1 AND ownership_epoch=?11 AND revision=?12",
                            params![
                                record.client_session_id.0,
                                schema_version,
                                record.protocol.0,
                                record.core_session_id.as_str(),
                                record.connection_id.as_str(),
                                ownership_epoch,
                                revision,
                                state,
                                capability_json,
                                updated_at_ms,
                                expected_epoch,
                                expected_revision,
                            ],
                        )
                        .map_err(database_error)?;
                    write_outcome(connection, &record.client_session_id, changed)
                },
            )
            .await
            .map_err(|error| AdapterError::HostUnavailable(error.to_string()))?
    }

    async fn remove_if_owner(
        &self,
        client_session_id: &ClientSessionId,
        expected_epoch: u64,
        expected_revision: u64,
    ) -> Result<RegistryWriteOutcome, AdapterError> {
        let client_session_id = client_session_id.clone();
        self.store
            .database()
            .call(
                move |connection| -> Result<RegistryWriteOutcome, AdapterError> {
                    let expected_epoch = stored_i64(expected_epoch, "expected_epoch")?;
                    let expected_revision = stored_i64(expected_revision, "expected_revision")?;
                    let changed = connection
                        .execute(
                            "DELETE FROM client_adapter_sessions WHERE client_session_id=?1 \
                         AND ownership_epoch=?2 AND revision=?3",
                            params![client_session_id.0, expected_epoch, expected_revision],
                        )
                        .map_err(database_error)?;
                    write_outcome(connection, &client_session_id, changed)
                },
            )
            .await
            .map_err(|error| AdapterError::HostUnavailable(error.to_string()))?
    }
}

fn write_outcome(
    connection: &rusqlite::Connection,
    id: &ClientSessionId,
    changed: usize,
) -> Result<RegistryWriteOutcome, AdapterError> {
    if changed == 1 {
        Ok(RegistryWriteOutcome::Applied)
    } else {
        Ok(RegistryWriteOutcome::Conflict(Box::new(load_optional(
            connection, id,
        )?)))
    }
}

fn load_optional(
    connection: &rusqlite::Connection,
    id: &ClientSessionId,
) -> Result<Option<ClientSessionRecord>, AdapterError> {
    use rusqlite::OptionalExtension;
    connection
        .query_row(
            "SELECT client_session_id, schema_version, protocol, core_session_id, \
             connection_id, ownership_epoch, revision, state, capability_json, updated_at_ms \
             FROM client_adapter_sessions WHERE client_session_id=?1",
            params![id.0],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .map(decode_record)
        .transpose()
}

fn load_one(
    connection: &rusqlite::Connection,
    id: &ClientSessionId,
) -> Result<ClientSessionRecord, AdapterError> {
    load_optional(connection, id)?
        .ok_or_else(|| AdapterError::HostUnavailable("conflicting registry row disappeared".into()))
}

fn decode_record(
    row: (
        String,
        i64,
        String,
        String,
        String,
        i64,
        i64,
        String,
        String,
        i64,
    ),
) -> Result<ClientSessionRecord, AdapterError> {
    let (
        client_session_id,
        schema,
        protocol,
        core_session,
        connection,
        epoch,
        revision,
        state,
        capabilities,
        updated,
    ) = row;
    Ok(ClientSessionRecord {
        schema_version: stored_u32(schema, "schema_version")?,
        protocol: ClientProtocol::new(protocol),
        client_session_id: ClientSessionId::new(client_session_id),
        core_session_id: SessionId::from(core_session),
        connection_id: ConnectionId::from(connection),
        ownership_epoch: stored_u64(epoch, "ownership_epoch")?,
        revision: stored_u64(revision, "revision")?,
        state: serde_json::from_str::<ClientSessionState>(&state)
            .map_err(|error| AdapterError::HostUnavailable(error.to_string()))?,
        capabilities: serde_json::from_str::<CapabilitySnapshot>(&capabilities)
            .map_err(|error| AdapterError::HostUnavailable(error.to_string()))?,
        updated_at: Timestamp::from_unix_millis(stored_u64(updated, "updated_at")?),
    })
}

fn stored_i64(value: u64, field: &str) -> Result<i64, AdapterError> {
    i64::try_from(value)
        .map_err(|_| AdapterError::HostUnavailable(format!("{field} exceeds SQLite range")))
}

fn stored_u64(value: i64, field: &str) -> Result<u64, AdapterError> {
    u64::try_from(value)
        .map_err(|_| AdapterError::HostUnavailable(format!("invalid negative {field}")))
}

fn stored_u32(value: i64, field: &str) -> Result<u32, AdapterError> {
    u32::try_from(value).map_err(|_| AdapterError::HostUnavailable(format!("invalid {field}")))
}

fn database_error(error: rusqlite::Error) -> AdapterError {
    AdapterError::HostUnavailable(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use client_adapter_api::{ClientCapability, SessionRegistry, CLIENT_ADAPTER_SCHEMA_VERSION};
    use rusqlite::OptionalExtension;
    use tempfile::tempdir;

    use super::*;

    fn record() -> ClientSessionRecord {
        ClientSessionRecord {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            protocol: ClientProtocol::new("acp"),
            client_session_id: ClientSessionId::new("external-1"),
            core_session_id: SessionId::from("core-1"),
            connection_id: ConnectionId::from("connection-1"),
            ownership_epoch: 1,
            revision: 3,
            state: ClientSessionState::Subscribed,
            capabilities: CapabilitySnapshot {
                schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
                protocol: ClientProtocol::new("acp"),
                protocol_version: "1".into(),
                client_version: "test".into(),
                revision: 1,
                capabilities: [ClientCapability::new("events")].into_iter().collect(),
            },
            updated_at: Timestamp::from_unix_millis(10),
        }
    }

    #[tokio::test]
    async fn registry_survives_store_reopen() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("sessions.sqlite3");
        let (store, _) = SessionStore::open(&path).await.expect("open");
        let backend: Arc<dyn SessionRegistryStore> =
            Arc::new(SqliteClientSessionRegistryStore::new(store.clone()));
        let registry = SessionRegistry::new(backend).await.expect("registry");
        registry.register(record()).await.expect("register");
        drop(registry);
        store.shutdown().await.expect("shutdown");

        let (reopened, report) = SessionStore::open(&path).await.expect("reopen");
        assert!(report.applied_versions.is_empty());
        let backend: Arc<dyn SessionRegistryStore> =
            Arc::new(SqliteClientSessionRegistryStore::new(reopened));
        let registry = SessionRegistry::new(backend).await.expect("reload");
        assert_eq!(
            registry.get(&ClientSessionId::new("external-1")).await,
            Some(record())
        );
    }

    #[tokio::test]
    async fn migration_seven_is_idempotent() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("sessions.sqlite3");
        let (store, report) = SessionStore::open(&path).await.expect("open");
        assert_eq!(report.to_version, 7);
        assert!(report.applied_versions.contains(&7));
        let exists: Option<i64> = store
            .database()
            .call(|connection| {
                connection
                    .query_row(
                        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='client_adapter_sessions'",
                        [],
                        |row| row.get(0),
                    )
                    .optional()
            })
            .await
            .expect("actor")
            .expect("query");
        assert_eq!(exists, Some(1));
    }

    #[tokio::test]
    async fn sqlite_compare_and_swap_rejects_competing_registry() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("sessions.sqlite3");
        let (store, _) = SessionStore::open(&path).await.expect("open");
        let backend: Arc<dyn SessionRegistryStore> =
            Arc::new(SqliteClientSessionRegistryStore::new(store));
        let first = SessionRegistry::new(backend.clone()).await.expect("first");
        first.register(record()).await.expect("register");
        let second = SessionRegistry::new(backend).await.expect("second");

        let claimed = first
            .claim(
                &ClientSessionId::new("external-1"),
                1,
                3,
                ConnectionId::from("connection-2"),
                ClientSessionState::Executing,
                Timestamp::from_unix_millis(11),
            )
            .await
            .expect("claim");
        assert!(matches!(
            second
                .transition(
                    &ClientSessionId::new("external-1"),
                    1,
                    3,
                    ClientSessionState::Disconnected,
                    Timestamp::from_unix_millis(12),
                )
                .await,
            Err(AdapterError::StaleOwner { .. })
        ));
        assert_eq!(
            second.get(&ClientSessionId::new("external-1")).await,
            Some(claimed)
        );
    }

    #[tokio::test]
    async fn sqlite_concurrent_cas_across_registries_advances_exactly_once() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("sessions.sqlite3");
        let (store, _) = SessionStore::open(&path).await.expect("open");
        let backend: Arc<dyn SessionRegistryStore> =
            Arc::new(SqliteClientSessionRegistryStore::new(store));
        let first = Arc::new(SessionRegistry::new(backend.clone()).await.expect("first"));
        first.register(record()).await.expect("register");
        let second = Arc::new(SessionRegistry::new(backend).await.expect("second"));

        // 两个 registry 独立缓存、共享同一 sqlite 后端：并发 claim 必须由
        // 原子 UPDATE ... WHERE epoch/revision 裁决，恰好一个成功。
        let mut tasks = Vec::new();
        for _ in 0..6 {
            for registry in [Arc::clone(&first), Arc::clone(&second)] {
                tasks.push(tokio::spawn(async move {
                    registry
                        .claim(
                            &ClientSessionId::new("external-1"),
                            1,
                            3,
                            ConnectionId::from("connection-2"),
                            ClientSessionState::Executing,
                            Timestamp::from_unix_millis(11),
                        )
                        .await
                }));
            }
        }

        let mut applied = 0usize;
        for task in tasks {
            match task.await.expect("join") {
                Ok(claimed) => {
                    applied += 1;
                    assert_eq!((claimed.ownership_epoch, claimed.revision), (2, 4));
                }
                Err(AdapterError::StaleOwner { .. }) => {}
                Err(other) => panic!("unexpected error: {other:?}"),
            }
        }
        assert_eq!(applied, 1, "exactly one claim may win the CAS");
        let current = first
            .get(&ClientSessionId::new("external-1"))
            .await
            .expect("authoritative record");
        assert_eq!((current.ownership_epoch, current.revision), (2, 4));
        assert_eq!(
            second.get(&ClientSessionId::new("external-1")).await,
            Some(current)
        );
    }

    #[tokio::test]
    async fn sqlite_remove_missing_row_returns_conflict_none() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("sessions.sqlite3");
        let (store, _) = SessionStore::open(&path).await.expect("open");
        let backend = SqliteClientSessionRegistryStore::new(store);

        // 行不存在：remove_if_owner 必须返回 Conflict(None) 而非假装成功。
        let outcome = backend
            .remove_if_owner(&ClientSessionId::new("never-existed"), 1, 1)
            .await
            .expect("store op");
        assert_eq!(outcome, RegistryWriteOutcome::Conflict(Box::new(None)));

        // ownership 不匹配：返回最新记录供重同步。
        backend.insert(&record()).await.expect("insert");
        let outcome = backend
            .remove_if_owner(&ClientSessionId::new("external-1"), 9, 9)
            .await
            .expect("store op");
        assert_eq!(
            outcome,
            RegistryWriteOutcome::Conflict(Box::new(Some(record())))
        );
    }

    #[tokio::test]
    async fn sqlite_remove_conflict_none_resyncs_registry() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("sessions.sqlite3");
        let (store, _) = SessionStore::open(&path).await.expect("open");
        let backend: Arc<dyn SessionRegistryStore> =
            Arc::new(SqliteClientSessionRegistryStore::new(store));
        let first = SessionRegistry::new(backend.clone()).await.expect("first");
        first.register(record()).await.expect("register");
        let second = SessionRegistry::new(backend.clone()).await.expect("second");
        let id = ClientSessionId::new("external-1");

        let removed = first.remove(&id, 1, 3).await.expect("first remove");
        assert_eq!((removed.ownership_epoch, removed.revision), (1, 3));
        assert_eq!(first.get(&id).await, None);

        // second 的缓存仍持有旧记录，但 store 行已删除：registry 层应同步
        // 清空缓存并返回 UnknownSession（Conflict(None) 路径）。
        assert!(matches!(
            second.remove(&id, 1, 3).await,
            Err(AdapterError::UnknownSession(_))
        ));
        assert_eq!(second.get(&id).await, None);

        // store 层最终确认无残留行。
        let outcome = backend.remove_if_owner(&id, 1, 3).await.expect("store op");
        assert_eq!(outcome, RegistryWriteOutcome::Conflict(Box::new(None)));
    }

}
