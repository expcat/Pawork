//! 异步文生视频目录与任务；独立于 Agent 事件和 Chat 流。
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VideoGenerationModel {
    pub id: String,
    pub display_name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VideoTaskStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Canceled,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VideoGenerationTask {
    pub id: String,
    pub model: String,
    pub status: VideoTaskStatus,
    pub url: Option<String>,
    pub error_code: Option<String>,
}
