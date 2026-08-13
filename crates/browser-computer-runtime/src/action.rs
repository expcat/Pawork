//! Canonical Browser / Computer action 与 snapshot（P17-10）。
//!
//! Agent 只见 canonical 描述与结果；后端差异封装在 trait 之后。action 可从
//! `ToolRequest.input`（JSON）解析，也可序列化回 JSON 用于审计。
use agent_domain::ArtifactReference;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::BrowserComputerError;

/// 统一的 Browser / Computer 动作（后端无关）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BrowserComputerAction {
    /// 导航到 URL。
    Navigate { url: String },
    /// 点击元素（按 selector 或屏幕坐标）。
    Click {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selector: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        coordinate: Option<(i32, i32)>,
    },
    /// 输入文本（可选目标 selector）。
    Type {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selector: Option<String>,
    },
    /// 按键 / 组合键。
    Key { keys: String },
    /// 滚动（像素增量）。
    Scroll {
        #[serde(default)]
        dx: i32,
        #[serde(default)]
        dy: i32,
    },
    /// 截图（结果以 artifact 引用返回，避免大 payload 进上下文）。
    Screenshot,
    /// 读取 DOM 文本（可选 selector；大输出折叠为 artifact）。
    SnapshotDom {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selector: Option<String>,
    },
    /// 读取页面标题。
    Title,
}

impl BrowserComputerAction {
    /// 从工具入参 JSON 解析 canonical action。
    pub fn from_input(input: &Value) -> Result<Self, BrowserComputerError> {
        let action = serde_json::from_value::<Self>(input.clone())
            .map_err(|err| BrowserComputerError::InvalidInput(err.to_string()))?;
        // Click 必须有 selector 或 coordinate；与 ToolDescriptor schema 的 anyOf 一致。
        if matches!(
            action,
            Self::Click {
                selector: None,
                coordinate: None
            }
        ) {
            return Err(BrowserComputerError::InvalidInput(
                "click requires `selector` or `coordinate`".into(),
            ));
        }
        Ok(action)
    }

    /// 是否为只读动作（仅观察，不改变页面状态）。
    ///
    /// 只读动作映射为 `ToolCapability::ReadOnly`，Policy 默认放行；其余动作视为
    /// 外部副作用（`Network`），按审批模式 gate。
    pub fn is_read_only(&self) -> bool {
        matches!(
            self,
            Self::Screenshot | Self::SnapshotDom { .. } | Self::Title
        )
    }

    /// 简短的可读标签（用于审计 / 日志，不含 secret）。
    pub fn label(&self) -> &'static str {
        match self {
            Self::Navigate { .. } => "navigate",
            Self::Click { .. } => "click",
            Self::Type { .. } => "type",
            Self::Key { .. } => "key",
            Self::Scroll { .. } => "scroll",
            Self::Screenshot => "screenshot",
            Self::SnapshotDom { .. } => "snapshot_dom",
            Self::Title => "title",
        }
    }
}

/// 统一的 Browser / Computer 结果快照。
///
/// 大字段（`dom`、原始截图字节）不应直接进入上下文；调用方经
/// [`crate::artifact::normalize_snapshot`] 把超阈值部分折叠为 `artifacts` 引用
/// （ADR-018），`dom` 仅保留短摘要。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BrowserComputerSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// DOM 文本（归一后应保持短；大输出折叠为 artifact 后置空）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dom: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactReference>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub metadata: Value,
}

impl BrowserComputerSnapshot {
    /// 以纯摘要构造一个最小快照。
    pub fn from_summary(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            ..Default::default()
        }
    }
}
