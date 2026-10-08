//! MCP 薄类型：与 `pawork-mcp` 同形的配置数据，不含 client / resolve / auth。
//!
//! 本模块只承载导入计划里的 MCP 声明形状，避免 workspace 导入层依赖
//! MCP runtime 或 `rmcp`。明文 Secret 不得进入这些类型。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

const DEFAULT_MAX_OUTPUT_BYTES: u64 = 1024 * 1024;
const DEFAULT_RESTART_MAX_ATTEMPTS: u32 = 1;
const DEFAULT_RESTART_BASE_DELAY_MS: u64 = 200;
const DEFAULT_RESTART_MAX_DELAY_MS: u64 = 10_000;
/// 导入计划 service 字符串的冻结副本，与
/// `pawork_auth::locator::MCP_SERVICE_PREFIX` 字节相同（含末尾点）。
/// 运行时命名空间判定只在 auth locator 的 `is_mcp_secret_service`；
/// 本常量不解析 Secret，也不是第二套门禁。
/// B8（2026-10-08）裁决保留此副本：workspace 不依赖 auth，
/// 否则 `pawork-tools` 的生产闭包会带上 reqwest / OAuth；
/// 不为这一字符串新建常量包，也不下沉到 domain。
/// 改值必须与 auth locator 同批，且不改变 `pawork.mcp.<server>` 导入字节。
const MCP_SECRET_SERVICE_PREFIX: &str = "pawork.mcp.";

pub(super) fn mcp_secret_service(server: &str) -> String {
    format!("{MCP_SECRET_SERVICE_PREFIX}{server}")
}

/// Locator for a plaintext secret held by a secret backend.
///
/// Only `service` and `account` are persisted/serialized — they are secret
/// backend locators, never the secret itself.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SecretRef {
    service: String,
    account: String,
}

impl SecretRef {
    /// Create a new secret reference from its backend locators.
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
        }
    }
}

/// One MCP server's configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub transport: TransportSpec,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub restart: RestartPolicy,
    #[serde(default)]
    pub permissions: McpPermissions,
    #[serde(default)]
    pub trusted: bool,
}

/// Transport configuration. Secret-bearing values are always [`SecretRef`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TransportSpec {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, SecretRef>,
    },
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, SecretRef>,
    },
}

/// Restart-on-crash policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartPolicy {
    #[serde(default = "default_restart_max_attempts")]
    pub max_attempts: u32,
    #[serde(default = "default_restart_base_delay_ms")]
    pub base_delay_ms: u64,
    #[serde(default = "default_restart_max_delay_ms")]
    pub max_delay_ms: u64,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            max_attempts: DEFAULT_RESTART_MAX_ATTEMPTS,
            base_delay_ms: DEFAULT_RESTART_BASE_DELAY_MS,
            max_delay_ms: DEFAULT_RESTART_MAX_DELAY_MS,
        }
    }
}

/// Per-server allowlists and output budget.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpPermissions {
    #[serde(default)]
    pub allowed_tools: BTreeSet<String>,
    #[serde(default)]
    pub allowed_workspaces: BTreeSet<String>,
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: u64,
}

impl Default for McpPermissions {
    fn default() -> Self {
        Self {
            allowed_tools: BTreeSet::new(),
            allowed_workspaces: BTreeSet::new(),
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

fn default_max_output_bytes() -> u64 {
    DEFAULT_MAX_OUTPUT_BYTES
}

fn default_restart_max_attempts() -> u32 {
    DEFAULT_RESTART_MAX_ATTEMPTS
}

fn default_restart_base_delay_ms() -> u64 {
    DEFAULT_RESTART_BASE_DELAY_MS
}

fn default_restart_max_delay_ms() -> u64 {
    DEFAULT_RESTART_MAX_DELAY_MS
}
