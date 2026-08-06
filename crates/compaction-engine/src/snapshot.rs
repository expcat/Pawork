//! [`CompactionSnapshot`] 版本化压缩摘要（P5-5）。
//!
//! 快照是压缩的唯一结构化产物：它记录被替换的事件区间、保留的事件 id、
//! 压缩前后的 token 统计，以及用于回退的 recovery branch。快照可 JSON 往返，
//! `version` 字段用于后续 schema 演进；读到不支持的版本时返回
//! [`UnsupportedSnapshotVersion`](crate::CompactionError::UnsupportedSnapshotVersion)。

use agent_domain::EventId;
use agent_events::EventSequence;
use serde::{Deserialize, Serialize};

use crate::CompactionError;

/// 当前 [`CompactionSnapshot`] schema 版本。
pub const CURRENT_SNAPSHOT_VERSION: u32 = 1;

/// 压缩快照的 schema 版本号。
///
/// 使用独立 newtype 而非裸 `u32`，避免与事件 `schema_version` 混淆，并让
/// 版本校验有明确边界。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SnapshotVersion(u32);

impl SnapshotVersion {
    /// 以原始数值构造版本号（仅用于测试或反序列化路径）。
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// 当前支持的版本。
    pub const fn current() -> Self {
        Self(CURRENT_SNAPSHOT_VERSION)
    }

    /// 原始版本数值。
    pub const fn value(self) -> u32 {
        self.0
    }
}

impl Default for SnapshotVersion {
    fn default() -> Self {
        Self::current()
    }
}

/// 一次压缩的结构化摘要。
///
/// - `version`：快照 schema 版本，校验失败返回 `UnsupportedSnapshotVersion`。
/// - `summary`：替代被压缩区间的自然语言摘要文本。
/// - `retained_event_ids`：压缩后仍逐字保留的事件 id（最近 N 轮、未解决任务等）。
/// - `replaced_range`：被折叠进摘要的连续事件序号区间（闭区间，起止均为
///   [`EventSequence`]）。
/// - `token_usage_before` / `token_usage_after`：压缩前后的输入 token 估算。
/// - `recovery_branch_id`：压缩前 Fork 的可恢复 branch id；`None` 表示未创建
///   recovery branch（如纯计算路径）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompactionSnapshot {
    pub version: SnapshotVersion,
    pub summary: String,
    pub retained_event_ids: Vec<EventId>,
    pub replaced_range: (EventSequence, EventSequence),
    pub token_usage_before: u64,
    pub token_usage_after: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_branch_id: Option<String>,
}

impl CompactionSnapshot {
    /// 校验 `version` 是否为当前支持的版本。
    pub fn validate(&self) -> Result<(), CompactionError> {
        if self.version == SnapshotVersion::current() {
            Ok(())
        } else {
            Err(CompactionError::UnsupportedSnapshotVersion {
                found: self.version.value(),
                supported: CURRENT_SNAPSHOT_VERSION,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CompactionSnapshot {
        CompactionSnapshot {
            version: SnapshotVersion::current(),
            summary: "前期讨论已折叠：用户要求实现压缩引擎。".into(),
            retained_event_ids: vec![EventId::from("event-7"), EventId::from("event-8")],
            replaced_range: (EventSequence::new(1), EventSequence::new(6)),
            token_usage_before: 12_000,
            token_usage_after: 3_000,
            recovery_branch_id: Some("compaction-recovery-main-6".into()),
        }
    }

    #[test]
    fn snapshot_round_trips_through_serde() {
        let snapshot = sample();
        let json = serde_json::to_string(&snapshot).expect("serialize");
        let back: CompactionSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, snapshot);
        assert_eq!(back.version, SnapshotVersion::current());
        back.validate().expect("current version validates");
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let mut snapshot = sample();
        snapshot.version = SnapshotVersion::new(999);
        match snapshot.validate() {
            Err(CompactionError::UnsupportedSnapshotVersion { found, supported }) => {
                assert_eq!(found, 999);
                assert_eq!(supported, CURRENT_SNAPSHOT_VERSION);
            }
            other => panic!("expected UnsupportedSnapshotVersion, got {other:?}"),
        }
    }

    #[test]
    fn recovery_branch_id_is_optional_and_skipped_when_none() {
        let mut snapshot = sample();
        snapshot.recovery_branch_id = None;
        let value = serde_json::to_value(&snapshot).expect("serialize");
        assert!(value.get("recovery_branch_id").is_none());
    }
}
