use agent_domain::{ArtifactId, CancellationToken, ServerToolEvent, ToolCallId};
use async_trait::async_trait;
use serde_json::Value;

use crate::action::{BrowserComputerAction, BrowserComputerSnapshot};
use crate::backend::{
    BackendKind, BackendProbe, BrowserComputerBackend, ExecutionSite, TrustBoundary,
};
use crate::error::BrowserComputerError;

/// Provider-hosted computer use 的事件发射面。
pub trait HostedComputerEventEmitter: Send + Sync {
    fn emit_action(
        &self,
        action: &BrowserComputerAction,
        tool_call_id: &ToolCallId,
    ) -> Vec<ServerToolEvent>;
}

/// Provider-hosted computer use 后端。
#[derive(Clone, Debug)]
pub struct ProviderHostedBackend {
    descriptor_name: &'static str,
    provider_label: &'static str,
    available: bool,
    unavailable_reason: String,
}

impl ProviderHostedBackend {
    pub fn new(provider_label: &'static str) -> Self {
        Self {
            descriptor_name: "browser_computer.provider_hosted",
            provider_label,
            available: true,
            unavailable_reason: String::new(),
        }
    }

    pub fn provider_label(&self) -> &'static str {
        self.provider_label
    }

    pub fn with_probe(mut self, available: bool, reason: impl Into<String>) -> Self {
        self.available = available;
        self.unavailable_reason = reason.into();
        self
    }
}
#[async_trait]
impl BrowserComputerBackend for ProviderHostedBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::ProviderHosted
    }
    fn execution_site(&self) -> ExecutionSite {
        ExecutionSite::ProviderHosted
    }
    fn trust_boundary(&self) -> TrustBoundary {
        TrustBoundary::ExternallyOwned
    }
    fn descriptor_name(&self) -> &'static str {
        self.descriptor_name
    }
    fn probe(&self) -> BackendProbe {
        if self.available {
            BackendProbe::available()
        } else {
            BackendProbe::unavailable(self.unavailable_reason.clone())
        }
    }

    async fn act(
        &self,
        _action: BrowserComputerAction,
        _workspace_id: &agent_domain::WorkspaceId,
        _cancel: CancellationToken,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        Err(BrowserComputerError::NotLocallyExecutable {
            backend: "provider_hosted",
            site: ExecutionSite::ProviderHosted.as_str(),
        })
    }

    async fn snapshot(
        &self,
        _workspace_id: &agent_domain::WorkspaceId,
        _cancel: CancellationToken,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        Err(BrowserComputerError::NotLocallyExecutable {
            backend: "provider_hosted",
            site: ExecutionSite::ProviderHosted.as_str(),
        })
    }
}

/// 默认事件发射器：把 canonical action 序列化为 ComputerActionRequested。
#[derive(Clone, Copy, Debug, Default)]
pub struct CanonicalHostedEmitter;

impl HostedComputerEventEmitter for CanonicalHostedEmitter {
    fn emit_action(
        &self,
        action: &BrowserComputerAction,
        tool_call_id: &ToolCallId,
    ) -> Vec<ServerToolEvent> {
        let payload: Value = serde_json::to_value(action).unwrap_or_default();
        vec![ServerToolEvent::ComputerActionRequested {
            tool_call_id: tool_call_id.clone(),
            action: payload,
        }]
    }
}

/// 构造一条 ComputerScreenshot 事件（供 provider 适配器回填截图 artifact）。
pub fn screenshot_event(
    tool_call_id: &ToolCallId,
    artifact: ArtifactId,
    media_type: Option<String>,
) -> ServerToolEvent {
    ServerToolEvent::ComputerScreenshot {
        tool_call_id: tool_call_id.clone(),
        artifact,
        media_type,
    }
}
