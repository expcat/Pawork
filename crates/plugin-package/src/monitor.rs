//! Monitor 子段的稳定 driver/evaluator 入口契约（P17-2，P16-10 延期接线）。
//!
//! Package manifest 的 `monitors` 子段**只声明**配置 / trigger / permissions /
//! lifecycle / required capability 与稳定 driver 入口；不重新定义运行时语义。
//! 实际执行统一进入 `monitor-service` → `task-manager`（P16-6 / P16-10）：
//!
//! - [`MonitorLifecycle`] 是单变体枚举（`TaskManager`），在类型层强制「package
//!   声明的 Monitor 以 `task-manager` 为唯一运行 lifecycle」——package 不自带
//!   运行时，也没有「绕过 task-manager 自托管 lifecycle」的选项。
//! - [`MonitorDriverEntry`] 指向稳定 driver/evaluator 入口（默认
//!   `monitor_service.evaluate`，即 `monitor-service` 的确定性纯函数判定）。
//! - `config` 是与 `monitor_service::MonitorConfig` 结构兼容的中性 JSON（tagged
//!   `kind`），由宿主在安装时反序列化为 `monitor_service::Monitor` 并注册。
//!
//! P16-10 已删除 `monitor-service` 内置 driver；真实 driver / executor 仍按
//! §2.3 延期。本 crate 只交付 package 可声明的驱动入口与归属生命周期契约。

use agent_domain::{MonitorId, MonitorSourceKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::PackageError;

/// Package 声明的 Monitor 的运行 lifecycle 归属。
///
/// 单变体：以 `task-manager` 为唯一运行 lifecycle（经 `monitor-service` 注册为
/// `TaskKind::Monitor`）。package 不得声明自托管 lifecycle。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorLifecycle {
    #[default]
    /// 经 `monitor-service` 注册到 `task-manager`（`TaskKind::Monitor`）。
    TaskManager,
}

/// 稳定 driver/evaluator 入口契约：package 声明的 Monitor 由谁判定。
///
/// `kind` 是稳定 driver 标识（默认 `monitor_service.evaluate`，即 monitor-service
/// 的确定性纯函数判定核心）；`config` 是与 `MonitorConfig` 兼容的 trigger config
/// （tagged `kind`），宿主据此构造 `monitor_service::Monitor` 并注册。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorDriverEntry {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub config: Value,
}

impl Default for MonitorDriverEntry {
    fn default() -> Self {
        Self {
            kind: Self::DEFAULT_KIND.to_string(),
            config: Value::Null,
        }
    }
}

impl MonitorDriverEntry {
    /// 默认入口：`monitor_service.evaluate`。
    pub const DEFAULT_KIND: &'static str = "monitor_service.evaluate";

    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            config: Value::Null,
        }
    }

    pub fn with_config(mut self, config: Value) -> Self {
        self.config = config;
        self
    }
}

/// Monitor 所需的宿主能力 / 权限声明（如 `fs`、`process`、`network`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorPermissions {
    /// 命中后允许触发的下游动作类别（`automation` / `webhook` 等），空表示仅记录。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_actions: Vec<String>,
    /// 允许访问的工作区相对路径或命名 scope（只读），空表示无额外权限。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filesystem_read: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network: Vec<String>,
}

/// Package 声明的 Monitor：稳定 driver 入口 + 唯一 lifecycle（task-manager）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorDeclaration {
    pub monitor_id: MonitorId,
    /// canonical 来源种类，与 `monitor_service::Monitor.source` 一致。
    pub source: MonitorSourceKind,
    /// 与 `monitor_service::MonitorConfig` 结构兼容的中性 JSON（tagged `kind`）。
    pub config: Value,
    /// 命中产出事件可喂养的 automation trigger（中性 JSON，可选）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<Value>,
    #[serde(default)]
    pub permissions: MonitorPermissions,
    /// 唯一 lifecycle：`task-manager`。默认即 `TaskManager`。
    #[serde(default)]
    pub lifecycle: MonitorLifecycle,
    /// driver 真正执行所需的宿主能力（如 `fs`、`process`）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_capability: Vec<String>,
    /// 稳定 driver/evaluator 入口。
    #[serde(default)]
    pub driver: MonitorDriverEntry,
}

impl MonitorDeclaration {
    /// 构造一个默认入口（`monitor_service.evaluate`）+ `task-manager` lifecycle 的声明。
    pub fn new(
        monitor_id: MonitorId,
        driver: MonitorDriverEntry,
        lifecycle: MonitorLifecycle,
    ) -> Self {
        let source =
            monitor_source_from_config(&driver.config).unwrap_or(MonitorSourceKind::FileChange);
        Self {
            monitor_id,
            source,
            config: driver.config.clone(),
            trigger: None,
            permissions: MonitorPermissions::default(),
            lifecycle,
            required_capability: Vec::new(),
            driver,
        }
    }

    /// 校验声明自洽：monitor_id 非空、driver kind 非空、config 是对象表（含 `kind`）。
    pub fn validate(&self) -> Result<(), PackageError> {
        if self.monitor_id.as_str().is_empty() {
            return Err(PackageError::field(
                "monitors",
                "monitor_id must not be empty",
            ));
        }
        if self.driver.kind.trim().is_empty() {
            return Err(PackageError::field(
                format!("monitors.{}", self.monitor_id.as_str()),
                "monitor driver kind must not be empty",
            ));
        }
        if self.config.is_null() {
            return Err(PackageError::field(
                format!("monitors.{}", self.monitor_id.as_str()),
                "monitor config must be provided (tagged `kind` table)",
            ));
        }
        let config_kind = self
            .config
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("");
        if config_kind.is_empty() {
            return Err(PackageError::field(
                format!("monitors.{}", self.monitor_id.as_str()),
                "monitor config must include a `kind` tag",
            ));
        }
        Ok(())
    }
}

/// 从 trigger config JSON 推导 canonical 来源种类（与 `MonitorConfig::source_kind` 对齐）。
fn monitor_source_from_config(config: &Value) -> Option<MonitorSourceKind> {
    let kind = config.get("kind")?.as_str()?;
    Some(match kind {
        "file_change" => MonitorSourceKind::FileChange,
        "process_exit" => MonitorSourceKind::ProcessExit,
        "regex_match" => MonitorSourceKind::RegexMatch,
        "port_state" => MonitorSourceKind::PortState,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lifecycle_default_is_task_manager() {
        assert_eq!(MonitorLifecycle::default(), MonitorLifecycle::TaskManager);
    }

    #[test]
    fn declaration_validates_config_kind_tag() {
        let mut decl = MonitorDeclaration::new(
            MonitorId::new("watch"),
            MonitorDriverEntry::new(MonitorDriverEntry::DEFAULT_KIND),
            MonitorLifecycle::TaskManager,
        )
        .clone();
        // config 缺失 → 报错。
        assert!(decl.validate().is_err());
        decl.config = json!({"kind": "file_change", "paths": ["a"]});
        decl.source = MonitorSourceKind::FileChange;
        decl.validate().expect("valid");
        assert_eq!(decl.source, MonitorSourceKind::FileChange);
    }

    #[test]
    fn declaration_rejects_empty_driver_kind() {
        let mut decl = MonitorDeclaration::new(
            MonitorId::new("watch"),
            MonitorDriverEntry::new("monitor_service.evaluate"),
            MonitorLifecycle::TaskManager,
        );
        decl.driver.kind = "  ".into();
        decl.config = json!({"kind": "port_state", "host": "127.0.0.1", "port": 8080});
        let err = decl.validate().unwrap_err();
        assert!(err.to_string().contains("driver kind"));
    }
}
