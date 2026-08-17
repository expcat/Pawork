//! 记忆领域模型：候选记忆与已存储记忆。

use pawork_domain::{EventId, MemoryId, MemoryPrivacy, WorkspaceId};

/// `extract` 产出的候选记忆：尚未嵌入、尚未分配 `MemoryId`。
///
/// 来源事件只读提炼，`source_event_id` 仅为引用线索，不持有事件所有权。
#[derive(Clone, Debug, PartialEq)]
pub struct CandidateMemory {
    pub summary: String,
    pub source_event_id: Option<EventId>,
    pub privacy: MemoryPrivacy,
    pub workspace_id: Option<WorkspaceId>,
    pub confidence: f32,
}

impl CandidateMemory {
    pub fn new(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            source_event_id: None,
            privacy: MemoryPrivacy::default(),
            workspace_id: None,
            confidence: 0.0,
        }
    }
}

/// 已存储的长期记忆。
#[derive(Clone, Debug)]
pub struct Memory {
    pub memory_id: MemoryId,
    pub summary: String,
    pub source_event_id: Option<EventId>,
    pub confidence: f32,
    pub privacy: MemoryPrivacy,
    pub workspace_id: Option<WorkspaceId>,
    /// Provider-neutral embedding；新 `Recorded` 事件携带向量，可经 replay 恢复。
    /// 旧流缺字段时默认空向量，并会被检索层过滤，需重新嵌入后才可检索。
    pub embedding: Vec<f32>,
    /// 是否有效；`invalidate` 置 `false`（不删除，保留可追溯）。
    pub valid: bool,
}
