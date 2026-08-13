//! MCP 后端：browser/computer MCP server。
//!
//! 执行位点按运行方式归为 `ClientFunction`（本地进程，CoreOwned）或
//! `ProviderExtension`（Provider 中介，ExternallyOwned）。位点由装配层在构造时
//! 显式声明，facade 不读 MCP server 或 Provider 名做分支。
use std::sync::Arc;
use std::sync::Mutex;

use agent_domain::{CancellationToken, WorkspaceId};
use async_trait::async_trait;
use sandbox_runtime::{
    SandboxBackend, SandboxError, SandboxPolicy, SandboxProcess, SandboxProcessSpec,
};

use crate::action::{BrowserComputerAction, BrowserComputerSnapshot};
use crate::backend::{
    BackendKind, BackendProbe, BrowserComputerBackend, ExecutionSite, TrustBoundary,
};
use crate::error::BrowserComputerError;
use crate::process::{ProcessMode, SandboxAuthorization, SandboxGate};

/// MCP browser/computer server 的运行方式（决定执行位点与信任边界）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpOwnership {
    /// 本地启动的 MCP 进程：`ClientFunction` + `CoreOwned`（经 sandbox spawn）。
    LocalProcess,
    /// Provider 中介的 MCP / connector：`ProviderExtension` + `ExternallyOwned`。
    ProviderMediated,
}

impl McpOwnership {
    pub const fn site(self) -> ExecutionSite {
        match self {
            Self::LocalProcess => ExecutionSite::ClientFunction,
            Self::ProviderMediated => ExecutionSite::ProviderExtension,
        }
    }

    pub const fn trust(self) -> TrustBoundary {
        match self {
            Self::LocalProcess => TrustBoundary::CoreOwned,
            Self::ProviderMediated => TrustBoundary::ExternallyOwned,
        }
    }
}

/// MCP server 调用面（可注入；真实实现复用 `mcp-client`）。
///
/// act/snapshot **必须**接收 [`SandboxAuthorization`]：授权由后端在调用 driver
/// 之前经 [`SandboxGate::acquire`] 发放，进程型携带 sandbox 句柄，preconnected
/// 显式不 spawn。
#[async_trait]
pub trait McpDriver: Send + Sync {
    async fn act(
        &self,
        action: BrowserComputerAction,
        workspace_id: &WorkspaceId,
        cancel: CancellationToken,
        authorization: SandboxAuthorization,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError>;
    async fn snapshot(
        &self,
        workspace_id: &WorkspaceId,
        cancel: CancellationToken,
        authorization: SandboxAuthorization,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError>;
}

/// MCP 后端。
#[derive(Clone)]
pub struct McpBackend {
    ownership: McpOwnership,
    driver: Arc<dyn McpDriver>,
    gate: Arc<Mutex<SandboxGate>>,
    descriptor_name: &'static str,
    available: bool,
    unavailable_reason: String,
}

impl McpBackend {
    /// 构造 MCP 后端。
    ///
    /// - `LocalProcess`：进程型，需注入 sandbox 后 spawn（未注入 fail closed）；
    /// - `ProviderMediated`：外部所有，in-process 闸门（Core 绝不为其 spawn）。
    pub fn new(ownership: McpOwnership, driver: Arc<dyn McpDriver>) -> Self {
        let descriptor_name: &'static str = match ownership {
            McpOwnership::LocalProcess => "browser_computer.mcp.local",
            McpOwnership::ProviderMediated => "browser_computer.mcp.extension",
        };
        let gate = match ownership {
            McpOwnership::LocalProcess => SandboxGate::spawn_required(),
            McpOwnership::ProviderMediated => SandboxGate::in_process(),
        };
        Self {
            ownership,
            driver,
            gate: Arc::new(Mutex::new(gate)),
            descriptor_name,
            available: true,
            unavailable_reason: String::new(),
        }
    }

    pub fn ownership(&self) -> McpOwnership {
        self.ownership
    }

    /// 声明为 preconnected（已连接的本地 MCP transport，不得再 spawn）。
    pub fn preconnected(self) -> Self {
        if self.ownership == McpOwnership::LocalProcess {
            *self.gate.lock().unwrap() = SandboxGate::preconnected();
        }
        self
    }

    /// 注入 Core-owned sandbox：本地 MCP server 子进程经 sandbox spawn。
    ///
    /// ProviderMediated（外部所有）不接受注入，Core 不为其 spawn 子进程。
    pub fn with_sandbox(self, sandbox: Arc<dyn SandboxBackend>) -> Self {
        if self.ownership == McpOwnership::LocalProcess {
            *self.gate.lock().unwrap() = SandboxGate::with_sandbox(sandbox);
        }
        self
    }

    /// 当前进程闸门模式。
    pub fn process_gate(&self) -> ProcessMode {
        self.gate.lock().unwrap().mode()
    }

    /// 已注入的 sandbox（供测试检视）。
    pub fn sandbox(&self) -> Option<Arc<dyn SandboxBackend>> {
        self.gate.lock().unwrap().sandbox().cloned()
    }

    /// 经注入 sandbox spawn 本地 MCP server 子进程；未注入 / 非进程型 fail closed。
    pub async fn spawn_server(
        &self,
        spec: SandboxProcessSpec,
        policy: SandboxPolicy,
        cancel: CancellationToken,
    ) -> Result<SandboxProcess, SandboxError> {
        // 复制闸门（Arc 克隆，廉价）后释放锁，避免持锁跨 await。
        let gate = self.gate.lock().unwrap().clone();
        gate.spawn(spec, policy, cancel).await
    }

    pub fn with_probe(mut self, available: bool, reason: impl Into<String>) -> Self {
        self.available = available;
        self.unavailable_reason = reason.into();
        self
    }
}

#[async_trait]
impl BrowserComputerBackend for McpBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Mcp
    }
    fn execution_site(&self) -> ExecutionSite {
        self.ownership.site()
    }
    fn trust_boundary(&self) -> TrustBoundary {
        self.ownership.trust()
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

    fn inject_sandbox(&self, sandbox: Arc<dyn SandboxBackend>) {
        if self.ownership == McpOwnership::LocalProcess {
            *self.gate.lock().unwrap() = SandboxGate::with_sandbox(sandbox);
        }
    }
    async fn act(
        &self,
        action: BrowserComputerAction,
        workspace_id: &WorkspaceId,
        cancel: CancellationToken,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        // ProviderExtension（外部所有）绝不本地执行；由 Provider transcript 续接。
        if self.ownership == McpOwnership::ProviderMediated {
            return Err(BrowserComputerError::NotLocallyExecutable {
                backend: "mcp",
                site: ExecutionSite::ProviderExtension.as_str(),
            });
        }
        // 因果闸门：Core-owned 本地 MCP 进程在 driver 可达前必须取得授权。
        let authorization = self.gate.lock().unwrap().acquire().map_err(|err| {
            BrowserComputerError::SandboxDenied {
                backend: "mcp",
                message: err.to_string(),
            }
        })?;
        self.driver
            .act(action, workspace_id, cancel, authorization)
            .await
    }
    async fn snapshot(
        &self,
        workspace_id: &WorkspaceId,
        cancel: CancellationToken,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        if self.ownership == McpOwnership::ProviderMediated {
            return Err(BrowserComputerError::NotLocallyExecutable {
                backend: "mcp",
                site: ExecutionSite::ProviderExtension.as_str(),
            });
        }
        let authorization = self.gate.lock().unwrap().acquire().map_err(|err| {
            BrowserComputerError::SandboxDenied {
                backend: "mcp",
                message: err.to_string(),
            }
        })?;
        self.driver
            .snapshot(workspace_id, cancel, authorization)
            .await
    }
}
