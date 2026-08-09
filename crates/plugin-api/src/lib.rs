//! Pawork WASM 插件的稳定宿主协议。
//!
//! 本 crate 只包含可序列化契约与进程内抽象，不实现 WASM、签名算法、存储或 IO。
//! 组件宿主位于 `wasm-plugin-host`，生命周期派发位于 `hook-runtime`。

mod invocation;
mod manifest;

pub use invocation::{
    PluginCommandInvocation, PluginInvocation, PluginInvocationOutput, PluginInvocationResponse,
    PluginOperation, PluginStateMutation, PluginStateScope, PluginStateSnapshot,
};
pub use manifest::{
    plugin_api_version, ManifestValidationError, PluginCapability, PluginCommandRegistration,
    PluginManifest, PluginPermissions, PluginSignature, PluginSignatureAlgorithm,
    PluginToolRegistration, SignedPluginManifest, MAX_PLUGIN_MANIFEST_BYTES, PLUGIN_API_VERSION,
    PLUGIN_INVOKE_EXPORT,
};

use agent_domain::{
    CancellationToken, CoreInstanceId, ErrorCategory, ErrorContext, RunId, SessionId, WorkspaceId,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginLifecycleEventKind {
    Load,
    Register,
    Start,
    Stop,
    Unload,
    CoreStart,
    WorkspaceOpen,
    SessionCreate,
    SessionOpen,
    RunStart,
    ContextBuild,
    ProviderRequest,
    AssistantDelta,
    ToolCall,
    ToolResult,
    Compaction,
    RunEnd,
    SessionClose,
    CoreShutdown,
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

impl PluginLifecycleEvent {
    pub const fn kind(&self) -> PluginLifecycleEventKind {
        match self {
            Self::Load => PluginLifecycleEventKind::Load,
            Self::Register => PluginLifecycleEventKind::Register,
            Self::Start => PluginLifecycleEventKind::Start,
            Self::Stop => PluginLifecycleEventKind::Stop,
            Self::Unload => PluginLifecycleEventKind::Unload,
            Self::CoreStart => PluginLifecycleEventKind::CoreStart,
            Self::WorkspaceOpen { .. } => PluginLifecycleEventKind::WorkspaceOpen,
            Self::SessionCreate { .. } => PluginLifecycleEventKind::SessionCreate,
            Self::SessionOpen { .. } => PluginLifecycleEventKind::SessionOpen,
            Self::RunStart { .. } => PluginLifecycleEventKind::RunStart,
            Self::ContextBuild { .. } => PluginLifecycleEventKind::ContextBuild,
            Self::ProviderRequest { .. } => PluginLifecycleEventKind::ProviderRequest,
            Self::AssistantDelta { .. } => PluginLifecycleEventKind::AssistantDelta,
            Self::ToolCall { .. } => PluginLifecycleEventKind::ToolCall,
            Self::ToolResult { .. } => PluginLifecycleEventKind::ToolResult,
            Self::Compaction { .. } => PluginLifecycleEventKind::Compaction,
            Self::RunEnd { .. } => PluginLifecycleEventKind::RunEnd,
            Self::SessionClose { .. } => PluginLifecycleEventKind::SessionClose,
            Self::CoreShutdown => PluginLifecycleEventKind::CoreShutdown,
        }
    }
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

impl PluginContext {
    pub fn state_scope(&self) -> PluginStateScope {
        if let Some(session_id) = &self.session_id {
            PluginStateScope::Session(session_id.clone())
        } else if let Some(workspace_id) = &self.workspace_id {
            PluginStateScope::Workspace(workspace_id.clone())
        } else {
            PluginStateScope::Global
        }
    }
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginErrorKind {
    InvalidManifest,
    SignatureRejected,
    IncompatibleApi,
    PermissionDenied,
    InvalidInvocation,
    State,
    FuelExhausted,
    MemoryLimit,
    Timeout,
    Cancelled,
    Trap,
    NotLoaded,
    Conflict,
    #[default]
    Internal,
}

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize, Deserialize)]
#[error("{kind:?}: {message}")]
pub struct PluginError {
    #[serde(default)]
    pub kind: PluginErrorKind,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
}

impl PluginError {
    pub fn new(kind: PluginErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retryable: false,
        }
    }

    pub fn cancelled(message: impl Into<String>) -> Self {
        Self::new(PluginErrorKind::Cancelled, message)
    }
}

impl From<PluginError> for ErrorContext {
    fn from(error: PluginError) -> Self {
        let category = match error.kind {
            PluginErrorKind::InvalidManifest
            | PluginErrorKind::IncompatibleApi
            | PluginErrorKind::InvalidInvocation => ErrorCategory::InvalidRequest,
            PluginErrorKind::SignatureRejected | PluginErrorKind::PermissionDenied => {
                ErrorCategory::Authorization
            }
            PluginErrorKind::Timeout => ErrorCategory::Timeout,
            PluginErrorKind::Cancelled => ErrorCategory::Cancelled,
            PluginErrorKind::NotLoaded => ErrorCategory::NotFound,
            PluginErrorKind::Conflict => ErrorCategory::Conflict,
            PluginErrorKind::State
            | PluginErrorKind::FuelExhausted
            | PluginErrorKind::MemoryLimit
            | PluginErrorKind::Trap => ErrorCategory::Tool,
            PluginErrorKind::Internal => ErrorCategory::Internal,
        };
        Self {
            category,
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
    fn permissions_default_to_deny() {
        let permissions = PluginPermissions::default();
        assert!(permissions.filesystem_read.is_empty());
        assert!(permissions.filesystem_write.is_empty());
        assert!(permissions.network.is_empty());
        assert!(!permissions.process);
        assert!(permissions.secret_refs.is_empty());
    }

    #[test]
    fn context_selects_most_specific_persistent_scope() {
        let context = PluginContext {
            instance_id: CoreInstanceId::from("core"),
            workspace_id: Some(WorkspaceId::from("workspace")),
            session_id: Some(SessionId::from("session")),
            run_id: Some(RunId::from("run")),
        };

        assert_eq!(
            context.state_scope(),
            PluginStateScope::Session(SessionId::from("session"))
        );
    }

    #[test]
    fn lifecycle_payload_maps_to_stable_kind() {
        let event = PluginLifecycleEvent::RunStart {
            run_id: RunId::from("run"),
        };
        assert_eq!(event.kind(), PluginLifecycleEventKind::RunStart);
    }

    #[test]
    fn plugin_errors_keep_actionable_shared_categories() {
        let context = ErrorContext::from(PluginError::new(
            PluginErrorKind::SignatureRejected,
            "untrusted signer",
        ));
        assert_eq!(context.category, ErrorCategory::Authorization);

        let context = ErrorContext::from(PluginError::new(
            PluginErrorKind::FuelExhausted,
            "fuel exhausted",
        ));
        assert_eq!(context.category, ErrorCategory::Tool);
    }
}
