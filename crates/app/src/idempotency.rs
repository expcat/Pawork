//! 命令幂等存储。
//!
//! 按 `(tenant, client_scope, command_id)` 与可选 `idempotency_key` 去重：
//! 网络重试携带相同标识时，返回首次响应缓存，绝不重复执行。
//! 持久态以 SQLite `command_ledger` 为准；进程内 `Notify` 仅唤醒 InFlight
//! 等待者，不算内存 CAS。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use pawork_domain::{CommandId, TenantId};
use pawork_protocol::{AppResponse, AppResponseEnvelope};
use pawork_storage::session::{
    CommandLedger, LedgerCheck, LedgerError, LedgerStats, DEFAULT_COMMAND_LEDGER_CAPACITY,
};
use thiserror::Error;
use tokio::sync::Notify;

/// 默认缓存容量（条目数）。
pub const DEFAULT_IDEMPOTENCY_CAPACITY: usize = DEFAULT_COMMAND_LEDGER_CAPACITY;

/// 幂等检查结果。
#[derive(Clone, Debug)]
pub enum IdempotencyCheck {
    /// 首次到达并已占位，应正常执行。
    New,
    /// 已处理过，重放首次响应。
    Replay(AppResponseEnvelope),
    /// 同标识正在执行，等待 `Notify` 后再 check。
    InFlight(Arc<Notify>),
}

/// 幂等存储统计。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IdempotencyStats {
    /// 已缓存条目数。
    pub entries: usize,
    /// 命中重放次数。
    pub replays: u64,
    /// 首次到达次数。
    pub new_commands: u64,
    /// 因容量上限被淘汰的条目数。
    pub evicted: u64,
    /// record 失败次数（冲突/关库等）。
    pub record_failures: u64,
}

#[derive(Debug, Error)]
pub enum IdempotencyError {
    #[error("command {0} was already recorded")]
    DuplicateCommand(String),
    #[error("idempotency key {key} is already bound to command {existing}")]
    KeyConflict { key: String, existing: String },
    #[error("command ledger is not available")]
    StoreUnavailable,
    #[error("command ledger closed")]
    Closed,
    #[error("{0}")]
    Other(String),
}

impl From<LedgerError> for IdempotencyError {
    fn from(error: LedgerError) -> Self {
        match error {
            LedgerError::DuplicateCommand(command) => Self::DuplicateCommand(command),
            LedgerError::KeyConflict { key, existing } => Self::KeyConflict { key, existing },
            LedgerError::Database(_) => Self::Closed,
            other => Self::Other(other.to_string()),
        }
    }
}

#[derive(Default)]
struct NotifyMap {
    waiters: HashMap<(String, String, String), Arc<Notify>>,
}

impl NotifyMap {
    fn key(tenant: &TenantId, scope: &str, command_id: &CommandId) -> (String, String, String) {
        (
            tenant.as_str().to_string(),
            scope.to_string(),
            command_id.as_str().to_string(),
        )
    }

    fn waiter(&mut self, tenant: &TenantId, scope: &str, command_id: &CommandId) -> Arc<Notify> {
        self.waiters
            .entry(Self::key(tenant, scope, command_id))
            .or_insert_with(|| Arc::new(Notify::new()))
            .clone()
    }

    fn notify(&mut self, tenant: &TenantId, scope: &str, command_id: &CommandId) {
        if let Some(notify) = self.waiters.remove(&Self::key(tenant, scope, command_id)) {
            notify.notify_waiters();
        }
    }
}

/// SQLite CommandLedger 封装。线程安全；`check` 与 `record` 分离以便先执行后缓存。
#[derive(Clone)]
pub struct IdempotencyStore {
    ledger: Option<CommandLedger>,
    client_scope: String,
    capacity: usize,
    waiters: Arc<Mutex<NotifyMap>>,
    counters: Arc<Mutex<IdempotencyStats>>,
}

impl IdempotencyStore {
    pub fn new(capacity: usize) -> Self {
        Self {
            ledger: None,
            client_scope: String::new(),
            capacity: capacity.max(1),
            waiters: Arc::new(Mutex::new(NotifyMap::default())),
            counters: Arc::new(Mutex::new(IdempotencyStats::default())),
        }
    }

    pub fn for_store(ledger: CommandLedger) -> Self {
        Self::for_store_with_scope(ledger, String::new(), DEFAULT_IDEMPOTENCY_CAPACITY)
    }

    pub fn for_store_with_scope(
        ledger: CommandLedger,
        client_scope: impl Into<String>,
        capacity: usize,
    ) -> Self {
        Self {
            ledger: Some(ledger),
            client_scope: client_scope.into(),
            capacity: capacity.max(1),
            waiters: Arc::new(Mutex::new(NotifyMap::default())),
            counters: Arc::new(Mutex::new(IdempotencyStats::default())),
        }
    }

    pub fn with_scope(&self, client_scope: impl Into<String>) -> Self {
        let mut clone = self.clone();
        clone.client_scope = client_scope.into();
        clone
    }

    /// 把进程内 InFlight Notify 接到 adapter 级共享 map。
    /// 持久态仍以 SQLite 为准；Notify 只唤醒，不算内存 CAS。
    pub fn share_waiters_from(&mut self, other: &Self) {
        self.waiters = Arc::clone(&other.waiters);
        self.counters = Arc::clone(&other.counters);
    }

    fn ledger(&self) -> Result<&CommandLedger, IdempotencyError> {
        self.ledger
            .as_ref()
            .ok_or(IdempotencyError::StoreUnavailable)
    }

    fn bump_replay(&self) {
        lock(&self.counters).replays += 1;
    }

    fn bump_new(&self) {
        lock(&self.counters).new_commands += 1;
    }

    pub fn bump_record_failure(&self) {
        lock(&self.counters).record_failures += 1;
    }

    /// 在指定 tenant + client_scope 内检查是否已处理过该命令。
    pub async fn check(
        &self,
        tenant_id: &TenantId,
        command_id: &CommandId,
        idempotency_key: Option<&str>,
    ) -> Result<IdempotencyCheck, IdempotencyError> {
        let ledger = self.ledger()?;
        match ledger
            .check(
                tenant_id.as_str(),
                &self.client_scope,
                command_id.as_str(),
                idempotency_key,
            )
            .await?
        {
            LedgerCheck::New => {
                self.bump_new();
                Ok(IdempotencyCheck::New)
            }
            LedgerCheck::Replay(json) => {
                self.bump_replay();
                let response = serde_json::from_str(&json)
                    .map_err(|error| IdempotencyError::Other(error.to_string()))?;
                Ok(IdempotencyCheck::Replay(response))
            }
            LedgerCheck::InFlight => {
                let notify = lock(&self.waiters).waiter(tenant_id, &self.client_scope, command_id);
                Ok(IdempotencyCheck::InFlight(notify))
            }
        }
    }

    /// 记录首次成功响应。仅在 `check` 返回 `New` 后调用；完成 inflight 占位。
    pub async fn record(
        &self,
        tenant_id: &TenantId,
        command_id: &CommandId,
        idempotency_key: Option<&str>,
        response: AppResponseEnvelope,
    ) -> Result<(), IdempotencyError> {
        let ledger = self.ledger()?;
        let json = serde_json::to_string(&response)
            .map_err(|error| IdempotencyError::Other(error.to_string()))?;
        let before = ledger.stats().await;
        let result = ledger
            .record(
                tenant_id.as_str(),
                &self.client_scope,
                command_id.as_str(),
                idempotency_key,
                &json,
                self.capacity,
            )
            .await;
        match result {
            Ok(()) => {
                let after = ledger.stats().await;
                if after.completed < before.completed + 1 {
                    lock(&self.counters).evicted += 1;
                }
                lock(&self.waiters).notify(tenant_id, &self.client_scope, command_id);
                Ok(())
            }
            Err(error) => {
                self.bump_record_failure();
                lock(&self.waiters).notify(tenant_id, &self.client_scope, command_id);
                Err(error.into())
            }
        }
    }

    /// 放弃 inflight 占位（dispatch 失败或错误响应不缓存）。
    pub async fn release(
        &self,
        tenant_id: &TenantId,
        command_id: &CommandId,
        idempotency_key: Option<&str>,
    ) {
        if let Some(ledger) = &self.ledger {
            if let Err(error) = ledger
                .release(
                    tenant_id.as_str(),
                    &self.client_scope,
                    command_id.as_str(),
                    idempotency_key,
                )
                .await
            {
                tracing::error!(
                    command_id = command_id.as_str(),
                    client_scope = self.client_scope.as_str(),
                    error = %error,
                    "command ledger release failed"
                );
            }
        }
        lock(&self.waiters).notify(tenant_id, &self.client_scope, command_id);
    }

    pub async fn stats(&self) -> IdempotencyStats {
        let mut stats = *lock(&self.counters);
        if let Some(ledger) = &self.ledger {
            let db: LedgerStats = ledger.stats().await;
            stats.entries = db.entries;
        }
        stats
    }
}

impl Default for IdempotencyStore {
    fn default() -> Self {
        Self::new(DEFAULT_IDEMPOTENCY_CAPACITY)
    }
}

fn lock<T>(inner: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 仅缓存非错误响应：错误响应不消耗幂等键，允许修复后重试。
pub fn should_cache(response: &AppResponseEnvelope) -> bool {
    !matches!(response.response, AppResponse::Error(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_domain::{QueryId, Timestamp};
    use pawork_protocol::{AppResponse, AppResponseEnvelope, API_VERSION};
    use pawork_storage::session::SessionStore;

    fn response(command_id: &str) -> AppResponseEnvelope {
        AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: QueryId::from(command_id),
            responded_at: Timestamp::from_unix_millis(1),
            response: AppResponse::Accepted {
                command_id: CommandId::from(command_id),
                run_id: None,
            },
        }
    }

    fn tenant(value: &str) -> TenantId {
        TenantId::new(value)
    }

    async fn temp_store(capacity: usize) -> (IdempotencyStore, SessionStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let (session, _) = SessionStore::open(dir.path().join("ledger.sqlite3"))
            .await
            .expect("open");
        let store = IdempotencyStore::for_store_with_scope(
            session.command_ledger(),
            String::new(),
            capacity,
        );
        (store, session, dir)
    }

    #[tokio::test]
    async fn same_command_id_replays_first_response() {
        let (store, session, _dir) = temp_store(16).await;
        let command_id = CommandId::from("cmd-1");
        assert!(matches!(
            store
                .check(&tenant("tenant-a"), &command_id, None)
                .await
                .expect("check"),
            IdempotencyCheck::New
        ));
        store
            .record(&tenant("tenant-a"), &command_id, None, response("cmd-1"))
            .await
            .expect("record");
        match store
            .check(&tenant("tenant-a"), &command_id, None)
            .await
            .expect("replay")
        {
            IdempotencyCheck::Replay(replay) => {
                assert_eq!(replay, response("cmd-1"));
            }
            other => panic!("expected replay, got {other:?}"),
        }
        session.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn idempotency_key_dedupes_across_command_ids() {
        let (store, session, _dir) = temp_store(16).await;
        let first = CommandId::from("cmd-1");
        let retry = CommandId::from("cmd-2");
        assert!(matches!(
            store
                .check(&tenant("tenant-a"), &first, Some("key-1"))
                .await
                .expect("check"),
            IdempotencyCheck::New
        ));
        store
            .record(
                &tenant("tenant-a"),
                &first,
                Some("key-1"),
                response("cmd-1"),
            )
            .await
            .expect("record");
        match store
            .check(&tenant("tenant-a"), &retry, Some("key-1"))
            .await
            .expect("key replay")
        {
            IdempotencyCheck::Replay(replay) => {
                assert_eq!(replay, response("cmd-1"), "重放首次响应");
            }
            other => panic!("expected replay via key, got {other:?}"),
        }
        session.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn capacity_evicts_oldest_entries() {
        let (store, session, _dir) = temp_store(2).await;
        for index in 0..3 {
            let id = CommandId::from(format!("cmd-{index}"));
            store
                .record(
                    &tenant("tenant-a"),
                    &id,
                    None,
                    response(&format!("cmd-{index}")),
                )
                .await
                .expect("record");
        }
        let stats = store.stats().await;
        assert_eq!(stats.entries, 2);
        assert_eq!(stats.evicted, 1);
        assert!(matches!(
            store
                .check(&tenant("tenant-a"), &CommandId::from("cmd-0"), None)
                .await
                .expect("evicted"),
            IdempotencyCheck::New
        ));
        assert!(matches!(
            store
                .check(&tenant("tenant-a"), &CommandId::from("cmd-2"), None)
                .await
                .expect("kept"),
            IdempotencyCheck::Replay(_)
        ));
        session.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn duplicate_record_and_key_conflict_are_rejected() {
        let (store, session, _dir) = temp_store(16).await;
        let first = CommandId::from("cmd-1");
        store
            .record(
                &tenant("tenant-a"),
                &first,
                Some("key-1"),
                response("cmd-1"),
            )
            .await
            .expect("record");
        assert!(matches!(
            store
                .record(&tenant("tenant-a"), &first, None, response("cmd-1"))
                .await,
            Err(IdempotencyError::DuplicateCommand(_))
        ));
        assert!(matches!(
            store
                .record(
                    &tenant("tenant-a"),
                    &CommandId::from("cmd-2"),
                    Some("key-1"),
                    response("cmd-2")
                )
                .await,
            Err(IdempotencyError::KeyConflict { .. })
        ));
        session.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn error_responses_are_not_cached() {
        let (store, session, _dir) = temp_store(16).await;
        let command_id = CommandId::from("cmd-1");
        let error = AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: QueryId::from("cmd-1"),
            responded_at: Timestamp::from_unix_millis(1),
            response: AppResponse::Error(pawork_domain::ErrorContext {
                category: pawork_domain::ErrorCategory::InvalidRequest,
                message: "bad".into(),
                retryable: false,
                retry_after_ms: None,
                diagnostics: std::collections::BTreeMap::new(),
            }),
        };
        assert!(!should_cache(&error));
        store
            .record(&tenant("tenant-a"), &command_id, Some("key-1"), error)
            .await
            .expect("record");
        assert_eq!(store.stats().await.entries, 1);
        session.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn tenants_can_reuse_command_and_idempotency_key_without_replay() {
        let (store, session, _dir) = temp_store(16).await;
        let command_id = CommandId::from("shared-command");
        store
            .record(
                &tenant("tenant-a"),
                &command_id,
                Some("shared-key"),
                response("tenant-a-response"),
            )
            .await
            .expect("tenant-a record");

        assert!(matches!(
            store
                .check(&tenant("tenant-b"), &command_id, Some("shared-key"))
                .await
                .expect("tenant-b new"),
            IdempotencyCheck::New
        ));
        store
            .record(
                &tenant("tenant-b"),
                &command_id,
                Some("shared-key"),
                response("tenant-b-response"),
            )
            .await
            .expect("tenant-b record");

        match store
            .check(
                &tenant("tenant-a"),
                &CommandId::from("tenant-a-retry"),
                Some("shared-key"),
            )
            .await
            .expect("tenant-a replay")
        {
            IdempotencyCheck::Replay(value) => {
                assert_eq!(value, response("tenant-a-response"));
            }
            other => panic!("tenant-a key should replay, got {other:?}"),
        }
        match store
            .check(
                &tenant("tenant-b"),
                &CommandId::from("tenant-b-retry"),
                Some("shared-key"),
            )
            .await
            .expect("tenant-b replay")
        {
            IdempotencyCheck::Replay(value) => {
                assert_eq!(value, response("tenant-b-response"));
            }
            other => panic!("tenant-b key should replay, got {other:?}"),
        }
        session.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn eviction_removes_only_the_evicted_tenant_key() {
        let (store, session, _dir) = temp_store(2).await;
        for tenant_id in ["tenant-a", "tenant-b"] {
            store
                .record(
                    &tenant(tenant_id),
                    &CommandId::from("shared-command"),
                    Some("shared-key"),
                    response(tenant_id),
                )
                .await
                .expect("record shared key");
        }
        store
            .record(
                &tenant("tenant-c"),
                &CommandId::from("command-c"),
                None,
                response("tenant-c"),
            )
            .await
            .expect("trigger eviction");

        let stats = store.stats().await;
        assert_eq!(stats.entries, 2);
        assert_eq!(stats.evicted, 1);
        assert!(matches!(
            store
                .check(
                    &tenant("tenant-a"),
                    &CommandId::from("retry-a"),
                    Some("shared-key")
                )
                .await
                .expect("evicted tenant-a"),
            IdempotencyCheck::New
        ));
        assert!(matches!(
            store
                .check(
                    &tenant("tenant-b"),
                    &CommandId::from("retry-b"),
                    Some("shared-key")
                )
                .await
                .expect("kept tenant-b"),
            IdempotencyCheck::Replay(_)
        ));
        session.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn check_cas_reserves_inflight_until_record_or_release() {
        let (store, session, _dir) = temp_store(16).await;
        let command_id = CommandId::from("cmd-1");
        assert!(matches!(
            store
                .check(&tenant("tenant-a"), &command_id, Some("key-1"))
                .await
                .expect("new"),
            IdempotencyCheck::New
        ));
        assert!(matches!(
            store
                .check(
                    &tenant("tenant-a"),
                    &CommandId::from("cmd-2"),
                    Some("key-1")
                )
                .await
                .expect("inflight"),
            IdempotencyCheck::InFlight(_)
        ));
        store
            .release(&tenant("tenant-a"), &command_id, Some("key-1"))
            .await;
        assert!(matches!(
            store
                .check(&tenant("tenant-a"), &command_id, Some("key-1"))
                .await
                .expect("after release"),
            IdempotencyCheck::New
        ));
        store
            .record(
                &tenant("tenant-a"),
                &command_id,
                Some("key-1"),
                response("cmd-1"),
            )
            .await
            .expect("record");
        assert!(matches!(
            store
                .check(
                    &tenant("tenant-a"),
                    &CommandId::from("cmd-3"),
                    Some("key-1")
                )
                .await
                .expect("replay"),
            IdempotencyCheck::Replay(_)
        ));
        session.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn missing_store_is_fail_closed() {
        let store = IdempotencyStore::new(16);
        let err = store
            .check(&tenant("tenant-a"), &CommandId::from("cmd-1"), None)
            .await
            .expect_err("no ledger");
        assert!(matches!(err, IdempotencyError::StoreUnavailable));
    }
}
