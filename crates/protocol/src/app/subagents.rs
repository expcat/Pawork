//! Subagent settings and list payloads (GUI API 1.20).
//!
//! Wire shapes match Global `[subagents]`. Unknown permission names stay
//! strings here; Host validates them. `AppResponse::Data` remains `Value`.

use serde::{Deserialize, Serialize};
#[cfg(feature = "typegen")]
use ts_rs::TS;

/// Default permission names when a model rule omits `permissions`.
///
/// An explicit empty list denies every permission.
pub const DEFAULT_SUBAGENT_PERMISSIONS: &[&str] = &[
    "read", "write", "terminal", "network", "mcp", "browser", "computer",
];

fn default_enabled() -> bool {
    true
}

fn default_max_concurrent() -> u16 {
    4
}

fn default_true() -> bool {
    true
}

fn default_subagent_permissions() -> Vec<String> {
    DEFAULT_SUBAGENT_PERMISSIONS
        .iter()
        .map(|name| (*name).to_string())
        .collect()
}

/// Global subagent settings (query Data / `SetSubagentSettings` params).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(default)]
pub struct SubagentSettingsData {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: u16,
    #[serde(default)]
    pub models: Vec<SubagentModelRule>,
}

impl Default for SubagentSettingsData {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            max_concurrent: default_max_concurrent(),
            models: Vec::new(),
        }
    }
}

/// Per-model spawn / subagent eligibility and permission names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(default)]
pub struct SubagentModelRule {
    pub provider_id: String,
    pub model_id: String,
    #[serde(default = "default_true")]
    pub allow_spawn: bool,
    #[serde(default = "default_true")]
    pub allow_as_subagent: bool,
    #[serde(default = "default_subagent_permissions")]
    pub permissions: Vec<String>,
    /// 子代理默认推理强度（canonical effort 名；ADR-063，API 1.21）。
    /// None = 回落模型级默认，再回落 Provider 默认。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    /// 子代理可选推理强度范围（空 = 不限；API 1.21）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_efforts: Vec<String>,
}

impl Default for SubagentModelRule {
    fn default() -> Self {
        Self {
            provider_id: String::new(),
            model_id: String::new(),
            allow_spawn: true,
            allow_as_subagent: true,
            permissions: default_subagent_permissions(),
            default_effort: None,
            allowed_efforts: Vec::new(),
        }
    }
}

/// `subagent_list` query Data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct SubagentListData {
    pub agents: Vec<SubagentInfo>,
}

/// One live or completed subagent row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct SubagentInfo {
    pub agent_id: String,
    pub session_id: String,
    pub parent_run_id: String,
    pub title: String,
    pub provider_id: String,
    pub model_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// spawn 时解析的生效推理强度（canonical effort 名；ADR-063，API 1.21）。
    /// None = 未显式配置（Provider 默认）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}
