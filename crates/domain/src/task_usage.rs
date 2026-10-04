//! 完整任务、章节 / 分段与操作类型的共同用量查询契约；不含正文或凭证。
use crate::{Cost, TokenUsage, VideoTaskStatus};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct UsageTaskRef {
    pub id: String,
    pub title: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum TaskUsageOperation {
    Text,
    Storyboard,
    Image,
    Video,
    Coding,
    Query,
    Download,
    Import,
    Export,
    Edit,
}

impl TaskUsageOperation {
    pub fn is_generation(self) -> bool {
        matches!(
            self,
            Self::Text | Self::Storyboard | Self::Image | Self::Video
        )
    }
    pub fn is_client_operation(self) -> bool {
        matches!(
            self,
            Self::Download | Self::Import | Self::Export | Self::Edit
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TaskUsageContext {
    pub task: UsageTaskRef,
    /// 小说分部等分组；与二级任务正交，不增加任务层级。
    pub group: Option<UsageTaskRef>,
    /// 小说章节或视频分段。
    pub subtask: Option<UsageTaskRef>,
    pub operation: TaskUsageOperation,
    /// 明确补交的原调用 ID；不覆盖原调用或推测自动重试。
    pub retry_of: Option<String>,
}

impl TaskUsageContext {
    pub fn validate(&self) -> bool {
        let valid_ref = |r: &UsageTaskRef| {
            valid_usage_id(&r.id)
                && !r.title.trim().is_empty()
                && r.title.len() <= 512
                && !r.title.chars().any(char::is_control)
        };
        valid_ref(&self.task)
            && self.group.as_ref().is_none_or(valid_ref)
            && self.subtask.as_ref().is_none_or(valid_ref)
            && self.retry_of.as_deref().is_none_or(valid_usage_id)
    }
}

pub fn valid_usage_id(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum TaskUsageStatus {
    Running,
    Submitted,
    Succeeded,
    Failed,
    Cancelled,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum TaskUsageSource {
    Gateway,
    NativeRun,
    ClientReport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum TaskUsageCostKind {
    Actual,
    Estimated,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(ts_rs::TS))]
pub struct TaskUsageCost {
    pub value: Cost,
    pub kind: TaskUsageCostKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(ts_rs::TS))]
pub struct TaskUsageRecord {
    /// Host 调用 ID；客户端报告使用独立命名空间。
    pub id: String,
    pub client: String,
    pub source: TaskUsageSource,
    /// 旧客户端没有显式关联时保留 None，查询仍可见。
    pub context: Option<TaskUsageContext>,
    pub operation: TaskUsageOperation,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub status: TaskUsageStatus,
    /// None 是未回传，不等于已确认的 0。
    pub tokens: Option<TokenUsage>,
    pub cost: Option<TaskUsageCost>,
    pub output_images: Option<u64>,
    /// 固定生成参数的计划值，不能当实际扣费或实测媒体时长。
    pub planned_video_seconds: Option<u64>,
    pub upstream_task_id: Option<String>,
    pub upstream_status: Option<VideoTaskStatus>,
    /// 状态查询 / 下载关联生成调用；不重复统计生成。
    pub related_call_id: Option<String>,
    /// 仅脱敏的分类码，不保存上游错误正文。
    pub error_code: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum TaskUsageGroupBy {
    #[default]
    Task,
    Group,
    Subtask,
    Operation,
    Model,
    Client,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TaskUsageCursor {
    pub started_at_ms: u64,
    pub client: String,
    pub id: String,
}

fn default_page_size() -> u32 {
    100
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(ts_rs::TS))]
#[serde(default, deny_unknown_fields)]
pub struct TaskUsageQuery {
    pub client: Option<String>,
    pub task_id: Option<String>,
    pub group_id: Option<String>,
    pub subtask_id: Option<String>,
    pub operation: Option<TaskUsageOperation>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub status: Option<TaskUsageStatus>,
    /// 时间半开区间 [start, end)。
    pub started_after_ms: Option<u64>,
    pub started_before_ms: Option<u64>,
    pub group_by: TaskUsageGroupBy,
    pub limit: u32,
    pub cursor: Option<TaskUsageCursor>,
}

impl Default for TaskUsageQuery {
    fn default() -> Self {
        Self {
            client: None,
            task_id: None,
            group_id: None,
            subtask_id: None,
            operation: None,
            provider: None,
            model: None,
            status: None,
            started_after_ms: None,
            started_before_ms: None,
            group_by: TaskUsageGroupBy::Task,
            limit: default_page_size(),
            cursor: None,
        }
    }
}

impl TaskUsageQuery {
    pub fn validate(&self) -> bool {
        (1..=200).contains(&self.limit)
            && [
                self.client.as_deref(),
                self.task_id.as_deref(),
                self.group_id.as_deref(),
                self.subtask_id.as_deref(),
                self.provider.as_deref(),
                self.model.as_deref(),
            ]
            .into_iter()
            .flatten()
            .all(valid_usage_id)
            && [
                self.started_after_ms,
                self.started_before_ms,
                self.cursor.as_ref().map(|c| c.started_at_ms),
            ]
            .into_iter()
            .flatten()
            .all(|v| v <= i64::MAX as u64)
            && !matches!((self.started_after_ms, self.started_before_ms), (Some(a),Some(b)) if a >= b)
            && self
                .cursor
                .as_ref()
                .is_none_or(|c| valid_usage_id(&c.id) && valid_usage_id(&c.client))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(ts_rs::TS))]
pub struct TaskUsageCurrencyTotal {
    pub currency: String,
    pub actual_micros: u64,
    pub estimated_micros: u64,
    pub actual_records: u64,
    pub estimated_records: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(ts_rs::TS))]
pub struct TaskUsageTotals {
    pub records: u64,
    pub generation_calls: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub unfinished: u64,
    /// 已知小计；必须与 unknown_token_calls 一起呈现。
    pub tokens: TokenUsage,
    pub unknown_token_calls: u64,
    pub unknown_cost_calls: u64,
    pub currencies: Vec<TaskUsageCurrencyTotal>,
    pub output_images: u64,
    pub planned_video_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(ts_rs::TS))]
pub struct TaskUsageGroup {
    pub key: String,
    pub title: String,
    pub totals: TaskUsageTotals,
    /// 精确下钻条件；未关联维度没有可用的下钻条件。
    pub filter: Option<TaskUsageQuery>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(ts_rs::TS))]
pub struct TaskUsageReport {
    /// 统计针对全部匹配记录，不受 limit / cursor 影响。
    pub totals: TaskUsageTotals,
    pub groups: Vec<TaskUsageGroup>,
    pub records: Vec<TaskUsageRecord>,
    pub next_cursor: Option<TaskUsageCursor>,
}

/// 本地下载 / 导入 / 导出 / 编辑的客户端报告；不接受费用或 Token。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TaskUsageOperationReport {
    pub report_id: String,
    pub context: TaskUsageContext,
    pub status: TaskUsageStatus,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub related_call_id: Option<String>,
}
