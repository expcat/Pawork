//! Playwright 后端：经子进程驱动浏览器。
//!
//! 执行位点 `ClientFunction`，信任边界 `CoreOwned`。子进程 spawn **必须**经注入的
//! `SandboxBackend`（ADR-031）：facade 装配时把 sandbox 注入后端，后端的
//! [`PlaywrightBackend::spawn_driver`] 把 playwright 驱动子进程交给沙箱执行，
//! 保证 Core-owned 后端不绕过 sandbox 直接起进程。
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

/// Playwright 高层控制面（可注入；真实实现经 [`PlaywrightBackend::spawn_driver`]
/// 与 playwright CLI 通信）。
///
/// act/snapshot **必须**接收 [`SandboxAuthorization`]：授权由后端在调用 driver
/// 之前经 [`SandboxGate::acquire`] 发放，进程型携带 sandbox 句柄，preconnected
/// 显式不 spawn。
#[async_trait]
pub trait PlaywrightDriver: Send + Sync {
    fn ready(&self) -> bool;
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

/// 默认 stub：playwright 未安装。`ready` 返回 false。
#[derive(Clone, Copy, Debug, Default)]
pub struct StubPlaywrightDriver;

#[async_trait]
impl PlaywrightDriver for StubPlaywrightDriver {
    fn ready(&self) -> bool {
        false
    }
    async fn act(
        &self,
        _action: BrowserComputerAction,
        _workspace_id: &WorkspaceId,
        _cancel: CancellationToken,
        _authorization: SandboxAuthorization,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        Err(BrowserComputerError::Backend {
            backend: "playwright",
            message: "playwright driver not installed".into(),
        })
    }
    async fn snapshot(
        &self,
        _workspace_id: &WorkspaceId,
        _cancel: CancellationToken,
        _authorization: SandboxAuthorization,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        Err(BrowserComputerError::Backend {
            backend: "playwright",
            message: "playwright driver not installed".into(),
        })
    }
}

/// Playwright 后端。
#[derive(Clone)]
pub struct PlaywrightBackend {
    driver: Arc<dyn PlaywrightDriver>,
    gate: Arc<Mutex<SandboxGate>>,
    descriptor_name: &'static str,
}

impl PlaywrightBackend {
    /// 以默认 stub 构造（进程型；无 sandbox 注入时 spawn fail closed）。
    pub fn new() -> Self {
        Self::with_driver(Arc::new(StubPlaywrightDriver))
    }

    pub fn with_driver(driver: Arc<dyn PlaywrightDriver>) -> Self {
        Self {
            driver,
            gate: Arc::new(Mutex::new(SandboxGate::spawn_required())),
            descriptor_name: "browser_computer.playwright",
        }
    }

    /// 声明为 preconnected（已连接既有 playwright 进程，不得再 spawn）。
    pub fn with_preconnected(self) -> Self {
        *self.gate.lock().unwrap() = SandboxGate::preconnected();
        self
    }

    /// 注入 Core-owned sandbox 后端（facade 装配时调用）。
    pub fn with_sandbox(self, sandbox: Arc<dyn SandboxBackend>) -> Self {
        *self.gate.lock().unwrap() = SandboxGate::with_sandbox(sandbox);
        self
    }

    /// 已注入的 sandbox（供审计/测试检视）。
    pub fn sandbox(&self) -> Option<Arc<dyn SandboxBackend>> {
        self.gate.lock().unwrap().sandbox().cloned()
    }

    /// 当前进程闸门模式。
    pub fn process_gate(&self) -> ProcessMode {
        self.gate.lock().unwrap().mode()
    }

    /// 经注入的 sandbox spawn playwright 驱动子进程。
    ///
    /// 未注入 sandbox 时报错——Core-owned 后端不得直接 spawn 绕过隔离。
    pub async fn spawn_driver(
        &self,
        spec: SandboxProcessSpec,
        policy: SandboxPolicy,
        cancel: CancellationToken,
    ) -> Result<SandboxProcess, SandboxError> {
        // 复制闸门（Arc 克隆，廉价）后释放锁，避免持锁跨 await。
        let gate = self.gate.lock().unwrap().clone();
        gate.spawn(spec, policy, cancel).await
    }
}

impl Default for PlaywrightBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BrowserComputerBackend for PlaywrightBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Playwright
    }
    fn execution_site(&self) -> ExecutionSite {
        ExecutionSite::ClientFunction
    }
    fn trust_boundary(&self) -> TrustBoundary {
        TrustBoundary::CoreOwned
    }
    fn descriptor_name(&self) -> &'static str {
        self.descriptor_name
    }
    fn probe(&self) -> BackendProbe {
        if self.driver.ready() {
            BackendProbe::available()
        } else {
            BackendProbe::unavailable("playwright driver not installed")
        }
    }

    fn inject_sandbox(&self, sandbox: Arc<dyn SandboxBackend>) {
        *self.gate.lock().unwrap() = SandboxGate::with_sandbox(sandbox);
    }
    async fn act(
        &self,
        action: BrowserComputerAction,
        workspace_id: &WorkspaceId,
        cancel: CancellationToken,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        // 因果闸门：driver 可达前必须取得执行授权（进程型未注入 sandbox → fail closed）。
        let authorization = self.gate.lock().unwrap().acquire().map_err(|err| {
            BrowserComputerError::SandboxDenied {
                backend: "playwright",
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
        let authorization = self.gate.lock().unwrap().acquire().map_err(|err| {
            BrowserComputerError::SandboxDenied {
                backend: "playwright",
                message: err.to_string(),
            }
        })?;
        self.driver
            .snapshot(workspace_id, cancel, authorization)
            .await
    }
}
