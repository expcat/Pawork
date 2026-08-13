use agent_domain::{CancellationToken, ToolKind, WorkspaceId};
use async_trait::async_trait;
use sandbox_runtime::SandboxBackend;
use std::sync::Arc;

use crate::action::{BrowserComputerAction, BrowserComputerSnapshot};
use crate::error::BrowserComputerError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BackendKind {
    Local,
    Playwright,
    Mcp,
    ProviderHosted,
}

impl BackendKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Playwright => "playwright",
            Self::Mcp => "mcp",
            Self::ProviderHosted => "provider_hosted",
        }
    }
}

/// Canonical 执行位点（承载 P15-1 `ToolKind` 语义）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExecutionSite {
    /// Core 本地执行（Local / Playwright / 本地 MCP 进程）。
    ClientFunction,
    /// Provider 中介的外部工具（Provider-mediated MCP / connector）。
    ProviderExtension,
    /// Provider 服务端内置 computer use。
    ProviderHosted,
}

impl ExecutionSite {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClientFunction => "client_function",
            Self::ProviderExtension => "provider_extension",
            Self::ProviderHosted => "provider_hosted",
        }
    }

    /// 对应的 canonical `ToolKind`。
    pub const fn tool_kind(self) -> ToolKind {
        match self {
            Self::ClientFunction => ToolKind::ClientFunction,
            Self::ProviderExtension => ToolKind::ProviderExtension,
            Self::ProviderHosted => ToolKind::ProviderHosted,
        }
    }
}
/// 信任边界：谁负责该后端的执行隔离。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TrustBoundary {
    /// Core 拥有，必须经 sandbox 隔离执行。
    CoreOwned,
    /// 外部所有，Core 只记录 trust boundary，不得标记为本地 sandboxed。
    ExternallyOwned,
}

impl TrustBoundary {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CoreOwned => "core_owned",
            Self::ExternallyOwned => "externally_owned",
        }
    }
}

/// 后端探测结果（探测须无副作用、可观察）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendProbe {
    pub available: bool,
    pub reason: String,
}

impl BackendProbe {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            available: false,
            reason: reason.into(),
        }
    }

    pub const fn available() -> Self {
        Self {
            available: true,
            reason: String::new(),
        }
    }
}
/// Browser / Computer 可替换后端。
///
/// `act` 与 `snapshot` 是本地（ClientFunction）执行路径。`ProviderHosted` 后端
/// 实现这两个方法时必须返回 `NotLocallyExecutable`——其生命周期走 ServerToolEvent，
/// 绝不进入本地 `AgentTool::execute()`。
#[async_trait]
pub trait BrowserComputerBackend: Send + Sync {
    fn kind(&self) -> BackendKind;
    fn execution_site(&self) -> ExecutionSite;
    fn trust_boundary(&self) -> TrustBoundary;
    fn descriptor_name(&self) -> &'static str;
    fn probe(&self) -> BackendProbe;

    /// 注入 Core-owned sandbox（facade 装配统一调用；进程型后端必须实现）。
    ///
    /// 默认 no-op；Local / Playwright / 本地 MCP 进程后端覆盖此方法把 sandbox
    /// 写入其 [`crate::process::SandboxGate`]。externally-owned 后端不接受注入。
    fn inject_sandbox(&self, _sandbox: Arc<dyn SandboxBackend>) {}

    async fn act(
        &self,
        action: BrowserComputerAction,
        workspace_id: &WorkspaceId,
        cancel: CancellationToken,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError>;

    async fn snapshot(
        &self,
        workspace_id: &WorkspaceId,
        cancel: CancellationToken,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError>;

    async fn teardown(&self) -> Result<(), BrowserComputerError> {
        Ok(())
    }
}

/// 后端路由摘要（由 selector 产生，进入审计记录）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendRoute {
    pub kind: BackendKind,
    pub site: ExecutionSite,
    pub trust: TrustBoundary,
    pub descriptor_name: &'static str,
}

impl BackendRoute {
    pub(crate) fn from_backend(backend: &dyn BrowserComputerBackend) -> Self {
        Self {
            kind: backend.kind(),
            site: backend.execution_site(),
            trust: backend.trust_boundary(),
            descriptor_name: backend.descriptor_name(),
        }
    }
}

/// 运行期硬门：任何非 `ClientFunction` 位点（ProviderHosted / ProviderExtension）
/// 从不得用于本地执行路径。
pub fn reject_non_client_function_for_local(
    backend: &dyn BrowserComputerBackend,
) -> Result<(), BrowserComputerError> {
    if backend.execution_site() != ExecutionSite::ClientFunction {
        return Err(BrowserComputerError::NotLocallyExecutable {
            backend: backend.kind().as_str(),
            site: backend.execution_site().as_str(),
        });
    }
    Ok(())
}

/// 兼容别名：只拒绝 ProviderHosted 的旧语义由
/// [`reject_non_client_function_for_local`] 统一覆盖。
pub fn reject_hosted_for_local(
    backend: &dyn BrowserComputerBackend,
) -> Result<(), BrowserComputerError> {
    reject_non_client_function_for_local(backend)
}
