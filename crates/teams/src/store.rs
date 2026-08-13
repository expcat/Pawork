//! Team 事件的 durable 持久化契约（append / replay）。
//!
//! [`TeamEventStore`] 是 Team 事件流的**可失败持久化层**：`append` 成功返回
//! 后，事件才允许被折叠进聚合、推进序列并镜像到上游（[`crate::service::TeamService`]
//! 保证 persist-first 语义）；`append` / `replay` 失败时命令面状态保持不变。
//!
//! 本 crate 只定义契约与测试用内存实现；生产 durable 后端（SQLite）由
//! `app-service` 的 `team` 模块实现并注入（依赖方向：`teams → app-service`，
//! teams 不持有任何存储后端）。

use std::sync::{Arc, Mutex};

use crate::error::TeamStoreError;
use crate::event::TeamEventEnvelope;
use crate::ids::TeamId;

/// Team 事件持久化层：append-only 追加 + 全量重放，均可失败。
///
/// 实现要点：
/// 1. `append` 必须原子落盘（同 `(team_id, sequence)` 重复追加返回
///    [`TeamStoreError::Duplicate`]），成功后调用方才推进内存序列。
/// 2. `replay` 返回按 `(team_id, sequence)` 升序的完整事件流，供重启重放
///    重建 [`crate::state::TeamAggregate`]。
/// 3. 实现必须 `Send + Sync`（命令面在多线程下调用）。
pub trait TeamEventStore: Send + Sync {
    /// 追加一条事件；失败时调用方状态不变。
    fn append(&self, envelope: &TeamEventEnvelope) -> Result<(), TeamStoreError>;
    /// 原子批量追加：全部成功或全部失败。
    ///
    /// 多事件命令（mailbox 批量投递、auto-retry 事件对、presence 批量派生）
    /// 依赖本接口维持 persist-first 语义：失败时**不得**留下部分已落盘事件。
    /// 默认实现按 [`Self::append`] 顺序追加（非原子，仅供测试双与过渡实现）；
    /// durable 后端（如 SQLite 事务）**必须**覆盖为单事务原子语义。
    fn append_batch(&self, envelopes: &[TeamEventEnvelope]) -> Result<(), TeamStoreError> {
        for envelope in envelopes {
            self.append(envelope)?;
        }
        Ok(())
    }
    /// 全量重放（按 team / 序列升序）；失败返回可诊断错误。
    fn replay(&self) -> Result<Vec<TeamEventEnvelope>, TeamStoreError>;
}

/// 校验重放流：每 team 内序列必须从 1 开始、严格连续（append-only 不变量）。
pub fn validate_sequence_contiguity(envelopes: &[TeamEventEnvelope]) -> Result<(), TeamStoreError> {
    let mut grouped: std::collections::BTreeMap<TeamId, Vec<u64>> =
        std::collections::BTreeMap::new();
    for envelope in envelopes {
        grouped
            .entry(envelope.team_id.clone())
            .or_default()
            .push(envelope.sequence.value());
    }
    for (team_id, mut sequences) in grouped {
        sequences.sort_unstable();
        // checked 推进期望序列：从 1 起逐条 +1；u64 溢出本身即视为损坏。
        let mut expected = 1u64;
        for found in sequences {
            if found != expected {
                return Err(TeamStoreError::NonContiguous {
                    team_id,
                    expected,
                    found,
                });
            }
            expected = expected.checked_add(1).ok_or_else(|| {
                TeamStoreError::Store(format!(
                    "sequence overflow while validating contiguity for team {team_id}"
                ))
            })?;
        }
    }
    Ok(())
}

/// 内存实现（默认装配 / 测试占位）：append 恒成功，replay 返回已追加事件。
#[derive(Default)]
pub struct MemoryTeamStore {
    events: Mutex<Vec<TeamEventEnvelope>>,
}

impl MemoryTeamStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 已持久化（追加）的事件，按追加顺序。
    pub fn events(&self) -> Vec<TeamEventEnvelope> {
        self.events.lock().expect("store poisoned").clone()
    }
}

impl TeamEventStore for MemoryTeamStore {
    fn append(&self, envelope: &TeamEventEnvelope) -> Result<(), TeamStoreError> {
        let mut events = self.events.lock().expect("store poisoned");
        if events
            .iter()
            .any(|e| e.team_id == envelope.team_id && e.sequence == envelope.sequence)
        {
            return Err(TeamStoreError::Duplicate(envelope.event_id.clone()));
        }
        events.push(envelope.clone());
        Ok(())
    }

    fn append_batch(&self, envelopes: &[TeamEventEnvelope]) -> Result<(), TeamStoreError> {
        let mut events = self.events.lock().expect("store poisoned");
        // 先整体校验再整体追加：任一重复即整批失败，不留部分写入。
        for envelope in envelopes {
            if events
                .iter()
                .any(|e| e.team_id == envelope.team_id && e.sequence == envelope.sequence)
            {
                return Err(TeamStoreError::Duplicate(envelope.event_id.clone()));
            }
        }
        events.extend(envelopes.iter().cloned());
        Ok(())
    }

    fn replay(&self) -> Result<Vec<TeamEventEnvelope>, TeamStoreError> {
        Ok(self.events())
    }
}

/// 把任意 `TeamEventStore` 提升为共享句柄。
pub fn shared_store<S: TeamEventStore + 'static>(store: S) -> Arc<dyn TeamEventStore> {
    Arc::new(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::TeamEvent;
    use crate::event::TeamEventSequence;
    use crate::ids::TeamId;
    use agent_domain::{AgentId, EventId, TenantId, Timestamp};

    fn envelope(team_id: &TeamId, sequence: u64) -> TeamEventEnvelope {
        TeamEventEnvelope::new(
            team_id.clone(),
            TeamEventSequence::new(sequence),
            EventId::new(format!("e-{sequence}")),
            Timestamp::from_unix_millis(1),
            TeamEvent::TeamCreated {
                team_id: team_id.clone(),
                tenant_id: TenantId::from("t"),
                supervisor: AgentId::from("s"),
                name: "T".into(),
            },
        )
    }

    #[test]
    fn memory_store_roundtrips_and_rejects_duplicates() {
        let store = MemoryTeamStore::new();
        let team = TeamId::from("t1");
        store.append(&envelope(&team, 1)).unwrap();
        store.append(&envelope(&team, 2)).unwrap();
        assert!(matches!(
            store.append(&envelope(&team, 1)),
            Err(TeamStoreError::Duplicate(_))
        ));
        let replayed = store.replay().unwrap();
        assert_eq!(replayed.len(), 2);
        assert_eq!(replayed[0].sequence.value(), 1);
        assert_eq!(replayed[1].sequence.value(), 2);
    }

    #[test]
    fn contiguity_validation_rejects_gaps_and_missing_first() {
        let team = TeamId::from("t1");
        assert!(validate_sequence_contiguity(&[]).is_ok());
        assert!(validate_sequence_contiguity(&[envelope(&team, 1), envelope(&team, 2)]).is_ok());
        // 缺首条。
        let err = validate_sequence_contiguity(&[envelope(&team, 2)]).unwrap_err();
        assert!(matches!(err, TeamStoreError::NonContiguous { .. }));
        // 断档。
        let err =
            validate_sequence_contiguity(&[envelope(&team, 1), envelope(&team, 3)]).unwrap_err();
        assert!(matches!(err, TeamStoreError::NonContiguous { .. }));
    }
}
