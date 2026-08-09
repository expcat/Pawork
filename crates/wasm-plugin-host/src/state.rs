//! 插件状态存储（P10-4）。
//!
//! 设计目标：
//! - **按 plugin + scope 隔离**：每个 `(PluginId, PluginStateScope)` 拥有独立的
//!   键空间，插件 A 永远读不到插件 B 的状态，同一插件的不同 scope（Global /
//!   Workspace / Session）也互不可见。
//! - **乐观并发控制**：每次 invoke 先快照 revision + values，apply mutations 时
//!   校验 `expected_revision == current`，否则返回 [`PluginStateError::RevisionMismatch`]。
//!   配合「每插件 Store 串行 invoke」即天然单写者，跨 scope / 跨插件天然隔离。
//! - **配额**：单值字节、单 scope 键数、单 scope 总字节上限由 [`HostConfig`] 注入，
//!   超限返回对应错误，防止状态被当作无界存储滥用。
//! - **可注入**：`PluginStateStore` 是 trait，默认实现 [`InMemoryPluginStateStore`]；
//!   durable backend 由组合层在 Secret/Policy 边界审查后注入，本 crate 不反向依赖数据库。

use std::collections::BTreeMap;
use std::sync::Arc;

use agent_domain::PluginId;
use parking_lot::RwLock;
use plugin_api::{PluginStateMutation, PluginStateScope, PluginStateSnapshot};
use serde_json::Value;

use crate::config::HostConfig;

/// 状态存储错误。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PluginStateError {
    #[error("state revision mismatch: expected {expected}, found {found}")]
    RevisionMismatch { expected: u64, found: u64 },
    #[error("state value too large: {got} bytes > {max}")]
    ValueTooLarge { got: usize, max: usize },
    #[error("state scope key limit exceeded: {got} > {max}")]
    TooManyKeys { got: usize, max: usize },
    #[error("state scope byte budget exceeded: {got} > {max}")]
    ScopeTooLarge { got: usize, max: usize },
    #[error("state key is empty")]
    EmptyKey,
    #[error("state size accounting overflow")]
    SizeOverflow,
    #[error("state revision overflow")]
    RevisionOverflow,
}

/// (plugin, scope) 复合键。两者都已实现 `Ord`/`Eq`/`Hash`，可直接作为 BTreeMap 键。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ScopeKey(PluginId, PluginStateScope);

#[derive(Clone, Debug, Default)]
struct ScopeBucket {
    revision: u64,
    values: BTreeMap<String, Value>,
}

/// 插件状态存储抽象。实现者负责隔离、并发与（可选的）持久化。
pub trait PluginStateStore: Send + Sync {
    /// 读取 `(plugin, scope)` 的快照（revision + 全部值）。
    fn snapshot(
        &self,
        plugin: &PluginId,
        scope: &PluginStateScope,
    ) -> Result<PluginStateSnapshot, PluginStateError>;

    /// 原子应用一组 mutations，要求 `expected_revision == current`。
    /// 成功返回新的 revision。
    fn apply(
        &self,
        plugin: &PluginId,
        scope: &PluginStateScope,
        mutations: &[PluginStateMutation],
        expected_revision: u64,
        config: &HostConfig,
    ) -> Result<u64, PluginStateError>;
}

/// 进程内默认实现：`Arc<RwLock<BTreeMap<ScopeKey, ScopeBucket>>>`。
///
/// 读多写少：`snapshot` 用读锁，`apply` 用写锁。隔离由复合键天然保证。
#[derive(Default)]
pub struct InMemoryPluginStateStore {
    buckets: Arc<RwLock<BTreeMap<ScopeKey, ScopeBucket>>>,
}

impl InMemoryPluginStateStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl PluginStateStore for InMemoryPluginStateStore {
    fn snapshot(
        &self,
        plugin: &PluginId,
        scope: &PluginStateScope,
    ) -> Result<PluginStateSnapshot, PluginStateError> {
        let buckets = self.buckets.read();
        let bucket = buckets
            .get(&ScopeKey(plugin.clone(), scope.clone()))
            .cloned()
            .unwrap_or_default();
        Ok(PluginStateSnapshot {
            revision: bucket.revision,
            values: bucket.values,
        })
    }

    fn apply(
        &self,
        plugin: &PluginId,
        scope: &PluginStateScope,
        mutations: &[PluginStateMutation],
        expected_revision: u64,
        config: &HostConfig,
    ) -> Result<u64, PluginStateError> {
        let mut buckets = self.buckets.write();
        let key = ScopeKey(plugin.clone(), scope.clone());
        let bucket = buckets.entry(key).or_default();

        if bucket.revision != expected_revision {
            return Err(PluginStateError::RevisionMismatch {
                expected: expected_revision,
                found: bucket.revision,
            });
        }

        // 先在一组工作副本上验证配额，全部通过后才提交，避免部分写入。
        let mut staged: BTreeMap<String, Value> = bucket.values.clone();
        for mutation in mutations {
            match mutation {
                PluginStateMutation::Set { key, value } => {
                    if key.trim().is_empty() {
                        return Err(PluginStateError::EmptyKey);
                    }
                    let value_bytes =
                        serde_json::to_vec(value).map_err(|_| PluginStateError::ValueTooLarge {
                            got: usize::MAX,
                            max: config.state_max_value_bytes,
                        })?;
                    if value_bytes.len() > config.state_max_value_bytes {
                        return Err(PluginStateError::ValueTooLarge {
                            got: value_bytes.len(),
                            max: config.state_max_value_bytes,
                        });
                    }
                    staged.insert(key.clone(), value.clone());
                }
                PluginStateMutation::Remove { key } => {
                    staged.remove(key);
                }
            }
        }

        if staged.len() > config.state_max_keys_per_scope {
            return Err(PluginStateError::TooManyKeys {
                got: staged.len(),
                max: config.state_max_keys_per_scope,
            });
        }
        let total = staged.iter().try_fold(0usize, |total, (key, value)| {
            let value_len = serde_json::to_vec(value)
                .map_err(|_| PluginStateError::SizeOverflow)?
                .len();
            total
                .checked_add(key.len())
                .and_then(|size| size.checked_add(value_len))
                .ok_or(PluginStateError::SizeOverflow)
        })?;
        if total > config.state_max_bytes_per_scope {
            return Err(PluginStateError::ScopeTooLarge {
                got: total,
                max: config.state_max_bytes_per_scope,
            });
        }

        let revision = bucket
            .revision
            .checked_add(1)
            .ok_or(PluginStateError::RevisionOverflow)?;
        bucket.values = staged;
        bucket.revision = revision;
        Ok(bucket.revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{SessionId, WorkspaceId};

    fn config() -> HostConfig {
        HostConfig {
            state_max_value_bytes: 32,
            state_max_keys_per_scope: 2,
            state_max_bytes_per_scope: 64,
            ..HostConfig::default()
        }
    }

    #[test]
    fn scopes_are_isolated_by_plugin_and_scope() {
        let store = InMemoryPluginStateStore::new();
        let cfg = config();
        let plugin_a = PluginId::from("a.plugin");
        let plugin_b = PluginId::from("b.plugin");
        let global = PluginStateScope::Global;
        let workspace = PluginStateScope::Workspace(WorkspaceId::from("w"));
        let session = PluginStateScope::Session(SessionId::from("s"));

        store
            .apply(
                &plugin_a,
                &global,
                &[PluginStateMutation::Set {
                    key: "k".into(),
                    value: Value::from(1),
                }],
                0,
                &cfg,
            )
            .unwrap();

        // 不同插件、不同 scope 都看不到 a/global 的写入。
        assert!(store
            .snapshot(&plugin_b, &global)
            .unwrap()
            .values
            .is_empty());
        assert!(store
            .snapshot(&plugin_a, &workspace)
            .unwrap()
            .values
            .is_empty());
        assert!(store
            .snapshot(&plugin_a, &session)
            .unwrap()
            .values
            .is_empty());
        assert_eq!(
            store.snapshot(&plugin_a, &global).unwrap().values["k"],
            Value::from(1)
        );
    }

    #[test]
    fn apply_enforces_optimistic_revision() {
        let store = InMemoryPluginStateStore::new();
        let cfg = config();
        let plugin = PluginId::from("a.plugin");
        let scope = PluginStateScope::Global;

        let rev = store
            .apply(
                &plugin,
                &scope,
                &[PluginStateMutation::Set {
                    key: "k".into(),
                    value: Value::from(1),
                }],
                0,
                &cfg,
            )
            .unwrap();
        assert_eq!(rev, 1);

        let err = store
            .apply(
                &plugin,
                &scope,
                &[PluginStateMutation::Set {
                    key: "k".into(),
                    value: Value::from(2),
                }],
                0,
                &cfg,
            )
            .unwrap_err();
        assert_eq!(
            err,
            PluginStateError::RevisionMismatch {
                expected: 0,
                found: 1
            }
        );
    }

    #[test]
    fn apply_enforces_value_and_scope_quotas() {
        let store = InMemoryPluginStateStore::new();
        let cfg = config();
        let plugin = PluginId::from("a.plugin");
        let scope = PluginStateScope::Global;

        let big = Value::from("x".repeat(64));
        let err = store
            .apply(
                &plugin,
                &scope,
                &[PluginStateMutation::Set {
                    key: "k".into(),
                    value: big,
                }],
                0,
                &cfg,
            )
            .unwrap_err();
        assert!(matches!(err, PluginStateError::ValueTooLarge { .. }));

        // 键数超限：上限 2，写入 3 个键（单值都小于等于 32 字节）。
        let errs = [
            ("a", Value::from(1)),
            ("b", Value::from(2)),
            ("c", Value::from(3)),
        ];
        let mut last = Ok(0);
        for (i, (key, value)) in errs.into_iter().enumerate() {
            last = store.apply(
                &plugin,
                &scope,
                &[PluginStateMutation::Set {
                    key: key.into(),
                    value,
                }],
                i as u64,
                &cfg,
            );
        }
        assert!(matches!(last, Err(PluginStateError::TooManyKeys { .. })));
    }
}
