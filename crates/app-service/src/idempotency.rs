//! 命令幂等存储（P13-1）。
//!
//! 按 `command_id` 与可选的 `idempotency_key` 去重：网络重试携带相同标识时，
//! 返回首次响应缓存，绝不重复执行（不会重复创建 Run / 消息 / Session）。
//! 缓存有界：超出容量后按插入顺序淘汰最旧条目。错误响应不缓存，允许客户端
//! 修复配置后用同一标识重试。

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use agent_domain::CommandId;
use core_api::AppResponse;
use core_api::AppResponseEnvelope;
use thiserror::Error;

/// 默认缓存容量（条目数）。
pub const DEFAULT_IDEMPOTENCY_CAPACITY: usize = 4096;

/// 幂等检查结果。
#[derive(Clone, Debug)]
pub enum IdempotencyCheck {
    /// 首次到达，应正常执行。
    New,
    /// 已处理过，重放首次响应。
    Replay(AppResponseEnvelope),
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

struct Inner {
    by_command: BTreeMap<CommandId, Entry>,
    by_key: BTreeMap<String, CommandId>,
    order: VecDeque<CommandId>,
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

    /// 检查是否已处理过该命令（按 command_id，其次按 idempotency_key）。
    pub fn check(&self, command_id: &CommandId, idempotency_key: Option<&str>) -> IdempotencyCheck {
        let mut inner = lock(&self.inner);
        if let Some(response) = inner
            .by_command
            .get(command_id)
            .map(|entry| entry.response.clone())
        {
            inner.replays += 1;
            return IdempotencyCheck::Replay(response);
        }
        if let Some(key) = idempotency_key {
            if let Some(original) = inner.by_key.get(key) {
                if let Some(response) = inner
                    .by_command
                    .get(original)
                    .map(|entry| entry.response.clone())
                {
                    inner.replays += 1;
                    return IdempotencyCheck::Replay(response);
                }
            }
        }
        IdempotencyCheck::New
    }

    /// 记录首次响应。仅在 `check` 返回 `New` 后调用。
    pub fn record(
        &self,
        command_id: &CommandId,
        idempotency_key: Option<&str>,
        response: AppResponseEnvelope,
    ) -> Result<(), IdempotencyError> {
        let mut inner = lock(&self.inner);
        if inner.by_command.contains_key(command_id) {
            return Err(IdempotencyError::DuplicateCommand(command_id.to_string()));
        }
        if let Some(key) = idempotency_key {
            if let Some(existing) = inner.by_key.get(key) {
                if existing != command_id {
                    return Err(IdempotencyError::KeyConflict {
                        key: key.to_string(),
                        existing: existing.to_string(),
                    });
                }
            }
            inner.by_key.insert(key.to_string(), command_id.clone());
        }
        inner.by_command.insert(
            command_id.clone(),
            Entry {
                response,
                idempotency_key: idempotency_key.map(str::to_string),
            },
        );
        inner.order.push_back(command_id.clone());
        inner.new_commands += 1;
        while inner.by_command.len() > self.capacity {
            let oldest = inner
                .order
                .pop_front()
                .expect("order length matches by_command length");
            if let Some(entry) = inner.by_command.remove(&oldest) {
                if let Some(key) = entry.idempotency_key {
                    inner.by_key.remove(&key);
                }
                inner.evicted += 1;
            }
        }
        Ok(())
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

/// 仅缓存非错误响应：错误响应不消耗幂等键，允许修复后重试。
pub fn should_cache(response: &AppResponseEnvelope) -> bool {
    !matches!(response.response, AppResponse::Error(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{QueryId, Timestamp};
    use core_api::{AppResponse, AppResponseEnvelope, API_VERSION};

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

    #[test]
    fn same_command_id_replays_first_response() {
        let store = IdempotencyStore::new(16);
        let command_id = CommandId::from("cmd-1");
        assert!(matches!(
            store.check(&command_id, None),
            IdempotencyCheck::New
        ));
        store
            .record(&command_id, None, response("cmd-1"))
            .expect("record");
        match store.check(&command_id, None) {
            IdempotencyCheck::Replay(replay) => {
                assert_eq!(replay, response("cmd-1"));
            }
            IdempotencyCheck::New => panic!("expected replay"),
        }
    }

    #[test]
    fn idempotency_key_dedupes_across_command_ids() {
        let store = IdempotencyStore::new(16);
        let first = CommandId::from("cmd-1");
        let retry = CommandId::from("cmd-2");
        assert!(matches!(
            store.check(&first, Some("key-1")),
            IdempotencyCheck::New
        ));
        store
            .record(&first, Some("key-1"), response("cmd-1"))
            .expect("record");
        match store.check(&retry, Some("key-1")) {
            IdempotencyCheck::Replay(replay) => {
                assert_eq!(replay, response("cmd-1"), "重放首次响应");
            }
            IdempotencyCheck::New => panic!("expected replay via key"),
        }
    }

    #[test]
    fn capacity_evicts_oldest_entries() {
        let store = IdempotencyStore::new(2);
        for index in 0..3 {
            let id = CommandId::from(format!("cmd-{index}"));
            store
                .record(&id, None, response(&format!("cmd-{index}")))
                .expect("record");
        }
        assert_eq!(store.stats().entries, 2);
        assert_eq!(store.stats().evicted, 1);
        assert!(matches!(
            store.check(&CommandId::from("cmd-0"), None),
            IdempotencyCheck::New
        ));
        assert!(matches!(
            store.check(&CommandId::from("cmd-2"), None),
            IdempotencyCheck::Replay(_)
        ));
    }

    #[test]
    fn duplicate_record_and_key_conflict_are_rejected() {
        let store = IdempotencyStore::new(16);
        let first = CommandId::from("cmd-1");
        store
            .record(&first, Some("key-1"), response("cmd-1"))
            .expect("record");
        assert!(matches!(
            store.record(&first, None, response("cmd-1")),
            Err(IdempotencyError::DuplicateCommand(_))
        ));
        assert!(matches!(
            store.record(&CommandId::from("cmd-2"), Some("key-1"), response("cmd-2")),
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
            response: AppResponse::Error(agent_domain::ErrorContext {
                category: agent_domain::ErrorCategory::InvalidRequest,
                message: "bad".into(),
                retryable: false,
                retry_after_ms: None,
                diagnostics: std::collections::BTreeMap::new(),
            }),
        };
        assert!(!should_cache(&error));
        store
            .record(&command_id, Some("key-1"), error)
            .expect("record");
        // 由调用方负责：error 响应不调用 record；此处验证 record 本身不拒绝。
        assert_eq!(store.stats().entries, 1);
    }
}
