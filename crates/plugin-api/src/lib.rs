//! Pawork 插件协议骨架。
//!
//! 本 crate 只冻结 manifest、能力声明与生命周期接口；WASM/MCP 宿主、加载器、
//! 签名验证和状态存储均不在此实现。

use agent_domain::{
    CancellationToken, CoreInstanceId, ErrorCategory, ErrorContext, PluginId, RunId, SessionId,
    WorkspaceId,
};
use async_trait::async_trait;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tool_api::ToolCapability;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: PluginId,
    pub name: String,
    pub version: Version,
    pub api_version: VersionReq,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub permissions: PluginPermissions,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<PluginCapability>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_capabilities: Vec<ToolCapability>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginPermissions {
    /// Workspace 相对路径或命名 scope；空列表表示无权限。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filesystem_read: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filesystem_write: Vec<String>,
    /// 允许访问的主机名；空列表表示无网络权限。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network: Vec<String>,
    #[serde(default)]
    pub process: bool,
    /// Secret 引用名，不是明文 Secret。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginCapability {
    RegisterTool,
    RegisterCommand,
    LifecycleHook,
    ModifyContext,
    CompactionStrategy,
    RegisterProvider,
    PersistentState,
    UserInteraction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginLifecycleEvent {
    Load,
    Register,
    Start,
    Stop,
    Unload,
    CoreStart,
    WorkspaceOpen { workspace_id: WorkspaceId },
    SessionCreate { session_id: SessionId },
    SessionOpen { session_id: SessionId },
    RunStart { run_id: RunId },
    ContextBuild { run_id: RunId },
    ProviderRequest { run_id: RunId },
    AssistantDelta { run_id: RunId },
    ToolCall { run_id: RunId },
    ToolResult { run_id: RunId },
    Compaction { session_id: SessionId },
    RunEnd { run_id: RunId },
    SessionClose { session_id: SessionId },
    CoreShutdown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginContext {
    pub instance_id: CoreInstanceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
}

#[async_trait]
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> &PluginManifest;

    async fn on_lifecycle_event(
        &self,
        event: PluginLifecycleEvent,
        context: PluginContext,
        cancel: CancellationToken,
    ) -> Result<(), PluginError>;
}

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize, Deserialize)]
#[error("{message}")]
pub struct PluginError {
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
}

impl From<PluginError> for ErrorContext {
    fn from(error: PluginError) -> Self {
        Self {
            category: ErrorCategory::Internal,
            message: error.message,
            retryable: error.retryable,
            retry_after_ms: None,
            diagnostics: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trip_preserves_tool_capabilities() {
        let manifest = PluginManifest {
            id: PluginId::from("example.plugin"),
            name: "Example".into(),
            version: Version::new(1, 2, 0),
            api_version: VersionReq::parse(">=1, <2").expect("valid version requirement"),
            description: None,
            permissions: PluginPermissions {
                filesystem_read: vec!["workspace".into()],
                network: vec!["api.example.com".into()],
                ..PluginPermissions::default()
            },
            capabilities: vec![PluginCapability::RegisterTool],
            tool_capabilities: vec![ToolCapability::ReadOnly],
        };

        let encoded = serde_json::to_string(&manifest).expect("serialize manifest");
        let decoded: PluginManifest = serde_json::from_str(&encoded).expect("deserialize manifest");
        assert_eq!(decoded, manifest);
    }

    #[test]
    fn permissions_default_to_deny() {
        let permissions = PluginPermissions::default();
        assert!(permissions.filesystem_read.is_empty());
        assert!(permissions.filesystem_write.is_empty());
        assert!(permissions.network.is_empty());
        assert!(!permissions.process);
        assert!(permissions.secret_refs.is_empty());
    }
}
