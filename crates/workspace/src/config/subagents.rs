//! Global-only subagent settings (`[subagents]`).
//!
//! Unknown permission names are accepted here and rejected by Host.
//! `max_concurrent` is bounded to 1..=16 at schema deserialize.

use serde::{Deserialize, Deserializer, Serialize};

/// Default concurrent subagent cap.
pub const DEFAULT_SUBAGENT_MAX_CONCURRENT: u16 = 4;
/// Inclusive lower bound for `max_concurrent`.
pub const SUBAGENT_MAX_CONCURRENT_MIN: u16 = 1;
/// Inclusive upper bound for `max_concurrent`.
pub const SUBAGENT_MAX_CONCURRENT_MAX: u16 = 16;

/// Default permission names when the key is omitted.
///
/// An explicit empty list denies every permission.
pub const DEFAULT_SUBAGENT_PERMISSIONS: &[&str] = &[
    "read", "write", "terminal", "network", "mcp", "browser", "computer",
];

/// `[subagents]` table stored only on Builtin/Global layers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubagentConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(
        default = "default_max_concurrent",
        deserialize_with = "deserialize_max_concurrent"
    )]
    pub max_concurrent: u16,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<SubagentModelConfig>,
}

impl Default for SubagentConfig {
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
#[serde(default)]
pub struct SubagentModelConfig {
    pub provider_id: String,
    pub model_id: String,
    #[serde(default = "default_true")]
    pub allow_spawn: bool,
    #[serde(default = "default_true")]
    pub allow_as_subagent: bool,
    #[serde(default = "default_subagent_permissions")]
    pub permissions: Vec<String>,
}

impl Default for SubagentModelConfig {
    fn default() -> Self {
        Self {
            provider_id: String::new(),
            model_id: String::new(),
            allow_spawn: true,
            allow_as_subagent: true,
            permissions: default_subagent_permissions(),
        }
    }
}

pub fn default_subagent_permissions() -> Vec<String> {
    DEFAULT_SUBAGENT_PERMISSIONS
        .iter()
        .map(|name| (*name).to_string())
        .collect()
}

fn default_enabled() -> bool {
    true
}

fn default_max_concurrent() -> u16 {
    DEFAULT_SUBAGENT_MAX_CONCURRENT
}

fn default_true() -> bool {
    true
}

fn deserialize_max_concurrent<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: Deserializer<'de>,
{
    let value = u16::deserialize(deserializer)?;
    if (SUBAGENT_MAX_CONCURRENT_MIN..=SUBAGENT_MAX_CONCURRENT_MAX).contains(&value) {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(format!(
            "max_concurrent must be {SUBAGENT_MAX_CONCURRENT_MIN}..={SUBAGENT_MAX_CONCURRENT_MAX}"
        )))
    }
}
