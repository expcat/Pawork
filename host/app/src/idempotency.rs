//! 命令幂等存储。
//!
//! 按 `command_id` 与可选的 `idempotency_key` 去重：网络重试携带相同标识时，
//! 返回首次响应缓存，绝不重复执行（不会重复创建 Run / 消息 / Session）。
//! `check` 以 CAS 占位（inflight / completed）；并发重试等完成或收到冲突。
//! 缓存有界：超出容量后按插入顺序淘汰最旧已完成条目。错误响应不缓存，允许
//! 客户端修复配置后用同一标识重试。

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use pawork_domain::{CommandId, TenantId};
use pawork_protocol::{AppResponse, AppResponseEnvelope};
use thiserror::Error;
use tokio::sync::Notify;

/// 默认缓存容量（条目数）。
pub const DEFAULT_IDEMPOTENCY_CAPACITY: usize = 4096;

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
}

#[derive(Debug, Error)]
pub enum IdempotencyError {
    #[error("command {0} was already recorded")]
    DuplicateCommand(String),
    #[error("idempotency key {key} is already bound to command {existing}")]
    KeyConflict { key: String, existing: String },
}

struct Entry {
    response: AppResponseEnvelope,
    idempotency_key: Option<String>,
}

enum Slot {
    Inflight {
        notify: Arc<Notify>,
        idempotency_key: Option<String>,
    },
    Completed(Entry),
}

struct Inner {
    by_command: BTreeMap<(TenantId, CommandId), Slot>,
    by_key: BTreeMap<(TenantId, String), CommandId>,
    order: VecDeque<(TenantId, CommandId)>,
    replays: u64,
    new_commands: u64,
    evicted: u64,
}

/// 有界幂等存储。线程安全；`check` 与 `record` 分离以便先执行后缓存。
pub struct IdempotencyStore {
    capacity: usize,
    inner: Mutex<Inner>,
}

impl IdempotencyStore {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            inner: Mutex::new(Inner {
                by_command: BTreeMap::new(),
                by_key: BTreeMap::new(),
                order: VecDeque::new(),
                replays: 0,
                new_commands: 0,
                evicted: 0,
            }),
        }
    }

    /// 在指定 tenant 内检查是否已处理过该命令（按 command_id，其次按
    /// idempotency_key）。相同标识可被不同 tenant 独立复用。
    pub fn check(
        &self,
        tenant_id: &TenantId,
        command_id: &CommandId,
        idempotency_key: Option<&str>,
    ) -> IdempotencyCheck {
        let mut inner = lock(&self.inner);
        let command_scope = (tenant_id.clone(), command_id.clone());
        if let Some(decision) = slot_decision(&inner, &command_scope) {
            if matches!(decision, IdempotencyCheck::Replay(_)) {
                inner.replays += 1;
            }
            return decision;
        }
        if let Some(key) = idempotency_key {
            let key_scope = (tenant_id.clone(), key.to_string());
            if let Some(original) = inner.by_key.get(&key_scope).cloned() {
                if let Some(decision) = slot_decision(&inner, &(tenant_id.clone(), original)) {
                    if matches!(decision, IdempotencyCheck::Replay(_)) {
                        inner.replays += 1;
                    }
                    return decision;
                }
            }
        }
        let notify = Arc::new(Notify::new());
        inner.by_command.insert(
            command_scope.clone(),
            Slot::Inflight {
                notify,
                idempotency_key: idempotency_key.map(str::to_string),
            },
        );
        if let Some(key) = idempotency_key {
            inner
                .by_key
                .insert((tenant_id.clone(), key.to_string()), command_id.clone());
        }
        inner.order.push_back(command_scope);
        inner.new_commands += 1;
        IdempotencyCheck::New
    }

    /// 记录首次成功响应。仅在 `check` 返回 `New` 后调用；完成 inflight 占位。
    pub fn record(
        &self,
        tenant_id: &TenantId,
        command_id: &CommandId,
        idempotency_key: Option<&str>,
        response: AppResponseEnvelope,
    ) -> Result<(), IdempotencyError> {
        let mut inner = lock(&self.inner);
        let command_scope = (tenant_id.clone(), command_id.clone());
        if let Some(Slot::Completed(_)) = inner.by_command.get(&command_scope) {
            return Err(IdempotencyError::DuplicateCommand(command_id.to_string()));
        }
        if let Some(key) = idempotency_key {
            let key_scope = (tenant_id.clone(), key.to_string());
            if let Some(existing) = inner.by_key.get(&key_scope) {
                if existing != command_id {
                    return Err(IdempotencyError::KeyConflict {
                        key: key.to_string(),
                        existing: existing.to_string(),
                    });
                }
            }
            inner.by_key.insert(key_scope, command_id.clone());
        }
        let notify = match inner.by_command.remove(&command_scope) {
            Some(Slot::Inflight { notify, .. }) => Some(notify),
            Some(Slot::Completed(entry)) => {
                inner.by_command.insert(command_scope, Slot::Completed(entry));
                return Err(IdempotencyError::DuplicateCommand(command_id.to_string()));
            }
            None => {
                inner.order.push_back(command_scope.clone());
                inner.new_commands += 1;
                None
            }
        };
        inner.by_command.insert(
            command_scope,
            Slot::Completed(Entry {
                response,
                idempotency_key: idempotency_key.map(str::to_string),
            }),
        );
        if let Some(notify) = notify {
            notify.notify_waiters();
        }
        evict_completed(&mut inner, self.capacity);
        Ok(())
    }

    /// 放弃 inflight 占位（dispatch 失败或错误响应不缓存）。
    pub fn release(
        &self,
        tenant_id: &TenantId,
        command_id: &CommandId,
        idempotency_key: Option<&str>,
    ) {
        let mut inner = lock(&self.inner);
        let command_scope = (tenant_id.clone(), command_id.clone());
        match inner.by_command.remove(&command_scope) {
            Some(Slot::Inflight { notify, idempotency_key: reserved_key }) => {
                inner.order.retain(|item| item != &command_scope);
                if let Some(key) = reserved_key.as_deref().or(idempotency_key) {
                    let key_scope = (tenant_id.clone(), key.to_string());
                    if inner.by_key.get(&key_scope) == Some(command_id) {
                        inner.by_key.remove(&key_scope);
                    }
                }
                notify.notify_waiters();
            }
            Some(other) => {
                inner.by_command.insert(command_scope, other);
            }
            None => {}
        }
    }

    pub fn stats(&self) -> IdempotencyStats {
        let inner = lock(&self.inner);
        IdempotencyStats {
            entries: inner.by_command.len(),
            replays: inner.replays,
            new_commands: inner.new_commands,
            evicted: inner.evicted,
        }
    }
}

impl Default for IdempotencyStore {
    fn default() -> Self {
        Self::new(DEFAULT_IDEMPOTENCY_CAPACITY)
    }
}

fn lock(inner: &Mutex<Inner>) -> std::sync::MutexGuard<'_, Inner> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn slot_decision(inner: &Inner, scope: &(TenantId, CommandId)) -> Option<IdempotencyCheck> {
    match inner.by_command.get(scope) {
        Some(Slot::Completed(entry)) => Some(IdempotencyCheck::Replay(entry.response.clone())),
        Some(Slot::Inflight { notify, .. }) => Some(IdempotencyCheck::InFlight(Arc::clone(notify))),
        None => None,
    }
}

fn evict_completed(inner: &mut Inner, capacity: usize) {
    while inner
        .by_command
        .values()
        .filter(|slot| matches!(slot, Slot::Completed(_)))
        .count()
        > capacity
    {
        let Some(oldest) = inner.order.iter().find(|scope| {
            matches!(inner.by_command.get(*scope), Some(Slot::Completed(_)))
        }).cloned() else {
            break;
        };
        inner.order.retain(|item| item != &oldest);
        if let Some(Slot::Completed(entry)) = inner.by_command.remove(&oldest) {
            if let Some(key) = entry.idempotency_key {
                inner.by_key.remove(&(oldest.0, key));
            }
            inner.evicted += 1;
        }
    }
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

    #[test]
    fn same_command_id_replays_first_response() {
        let store = IdempotencyStore::new(16);
        let command_id = CommandId::from("cmd-1");
        assert!(matches!(
            store.check(&tenant("tenant-a"), &command_id, None),
            IdempotencyCheck::New
        ));
        store
            .record(&tenant("tenant-a"), &command_id, None, response("cmd-1"))
            .expect("record");
        match store.check(&tenant("tenant-a"), &command_id, None) {
            IdempotencyCheck::Replay(replay) => {
                assert_eq!(replay, response("cmd-1"));
            }
            other => panic!("expected replay, got {other:?}"),
        }
    }

    #[test]
    fn idempotency_key_dedupes_across_command_ids() {
        let store = IdempotencyStore::new(16);
        let first = CommandId::from("cmd-1");
        let retry = CommandId::from("cmd-2");
        assert!(matches!(
            store.check(&tenant("tenant-a"), &first, Some("key-1")),
            IdempotencyCheck::New
        ));
        store
            .record(
                &tenant("tenant-a"),
                &first,
                Some("key-1"),
                response("cmd-1"),
            )
            .expect("record");
        match store.check(&tenant("tenant-a"), &retry, Some("key-1")) {
            IdempotencyCheck::Replay(replay) => {
                assert_eq!(replay, response("cmd-1"), "重放首次响应");
            }
            other => panic!("expected replay via key, got {other:?}"),
        }
    }

    #[test]
    fn capacity_evicts_oldest_entries() {
        let store = IdempotencyStore::new(2);
        for index in 0..3 {
            let id = CommandId::from(format!("cmd-{index}"));
            store
                .record(
                    &tenant("tenant-a"),
                    &id,
                    None,
                    response(&format!("cmd-{index}")),
                )
                .expect("record");
        }
        assert_eq!(store.stats().entries, 2);
        assert_eq!(store.stats().evicted, 1);
        assert!(matches!(
            store.check(&tenant("tenant-a"), &CommandId::from("cmd-0"), None),
            IdempotencyCheck::New
        ));
        assert!(matches!(
            store.check(&tenant("tenant-a"), &CommandId::from("cmd-2"), None),
            IdempotencyCheck::Replay(_)
        ));
    }

    #[test]
    fn duplicate_record_and_key_conflict_are_rejected() {
        let store = IdempotencyStore::new(16);
        let first = CommandId::from("cmd-1");
        store
            .record(
                &tenant("tenant-a"),
                &first,
                Some("key-1"),
                response("cmd-1"),
            )
            .expect("record");
        assert!(matches!(
            store.record(&tenant("tenant-a"), &first, None, response("cmd-1")),
            Err(IdempotencyError::DuplicateCommand(_))
        ));
        assert!(matches!(
            store.record(
                &tenant("tenant-a"),
                &CommandId::from("cmd-2"),
                Some("key-1"),
                response("cmd-2")
            ),
            Err(IdempotencyError::KeyConflict { .. })
        ));
    }

    #[test]
    fn error_responses_are_not_cached() {
        let store = IdempotencyStore::new(16);
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
            .expect("record");
        // 由调用方负责：error 响应不调用 record；此处验证 record 本身不拒绝。
        assert_eq!(store.stats().entries, 1);
    }

    #[test]
    fn tenants_can_reuse_command_and_idempotency_key_without_replay() {
        let store = IdempotencyStore::new(16);
        let command_id = CommandId::from("shared-command");
        store
            .record(
                &tenant("tenant-a"),
                &command_id,
                Some("shared-key"),
                response("tenant-a-response"),
            )
            .expect("tenant-a record");

        assert!(matches!(
            store.check(&tenant("tenant-b"), &command_id, Some("shared-key")),
            IdempotencyCheck::New
        ));
        store
            .record(
                &tenant("tenant-b"),
                &command_id,
                Some("shared-key"),
                response("tenant-b-response"),
            )
            .expect("tenant-b record");

        match store.check(
            &tenant("tenant-a"),
            &CommandId::from("tenant-a-retry"),
            Some("shared-key"),
        ) {
            IdempotencyCheck::Replay(value) => {
                assert_eq!(value, response("tenant-a-response"));
            }
            other => panic!("tenant-a key should replay, got {other:?}"),
        }
        match store.check(
            &tenant("tenant-b"),
            &CommandId::from("tenant-b-retry"),
            Some("shared-key"),
        ) {
            IdempotencyCheck::Replay(value) => {
                assert_eq!(value, response("tenant-b-response"));
            }
            other => panic!("tenant-b key should replay, got {other:?}"),
        }
    }

    #[test]
    fn eviction_removes_only_the_evicted_tenant_key() {
        let store = IdempotencyStore::new(2);
        for tenant_id in ["tenant-a", "tenant-b"] {
            store
                .record(
                    &tenant(tenant_id),
                    &CommandId::from("shared-command"),
                    Some("shared-key"),
                    response(tenant_id),
                )
                .expect("record shared key");
        }
        store
            .record(
                &tenant("tenant-c"),
                &CommandId::from("command-c"),
                None,
                response("tenant-c"),
            )
            .expect("trigger eviction");

        assert_eq!(store.stats().entries, 2);
        assert_eq!(store.stats().evicted, 1);
        assert!(matches!(
            store.check(
                &tenant("tenant-a"),
                &CommandId::from("retry-a"),
                Some("shared-key")
            ),
            IdempotencyCheck::New
        ));
        assert!(matches!(
            store.check(
                &tenant("tenant-b"),
                &CommandId::from("retry-b"),
                Some("shared-key")
            ),
            IdempotencyCheck::Replay(_)
        ));
    }

    #[test]
    fn check_cas_reserves_inflight_until_record_or_release() {
        let store = IdempotencyStore::new(16);
        let command_id = CommandId::from("cmd-1");
        assert!(matches!(
            store.check(&tenant("tenant-a"), &command_id, Some("key-1")),
            IdempotencyCheck::New
        ));
        assert!(matches!(
            store.check(&tenant("tenant-a"), &CommandId::from("cmd-2"), Some("key-1")),
            IdempotencyCheck::InFlight(_)
        ));
        store.release(&tenant("tenant-a"), &command_id, Some("key-1"));
        assert!(matches!(
            store.check(&tenant("tenant-a"), &command_id, Some("key-1")),
            IdempotencyCheck::New
        ));
        store
            .record(
                &tenant("tenant-a"),
                &command_id,
                Some("key-1"),
                response("cmd-1"),
            )
            .expect("record");
        assert!(matches!(
            store.check(&tenant("tenant-a"), &CommandId::from("cmd-3"), Some("key-1")),
            IdempotencyCheck::Replay(_)
        ));
    }
}
