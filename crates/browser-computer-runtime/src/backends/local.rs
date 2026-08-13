//! Local 后端：本地浏览器（in-process / 直连 CDP）。
//!
//! 执行位点 `ClientFunction`，信任边界 `CoreOwned`。控制面经可注入的
//! [`LocalDriver`] 实现；默认 [`StubLocalDriver`] 返回「未配置」错误，便于测试
//! 注入 recording driver。Core 拥有的浏览器驱动子进程应经注入的 sandbox 执行
//! （由 facade 统一装配，见 `BrowserComputerCapability`）。
use std::sync::Arc;

use agent_domain::{CancellationToken, WorkspaceId};
use async_trait::async_trait;
use sandbox_runtime::{
    SandboxBackend, SandboxError, SandboxPolicy, SandboxProcess, SandboxProcessSpec,
};
use std::sync::Mutex;

use crate::action::{BrowserComputerAction, BrowserComputerSnapshot};
use crate::backend::{
    BackendKind, BackendProbe, BrowserComputerBackend, ExecutionSite, TrustBoundary,
};
use crate::error::BrowserComputerError;
use crate::process::{ProcessMode, SandboxAuthorization, SandboxGate};

/// 本地浏览器控制面（可注入；测试用 recording 实现）。
///
/// act/snapshot **必须**接收 [`SandboxAuthorization`]：授权由后端在调用 driver
/// 之前经 [`SandboxGate::acquire`] 发放，进程型携带 sandbox 句柄，in-process /
/// preconnected 显式不 spawn。
#[async_trait]
pub trait LocalDriver: Send + Sync {
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

/// 默认 stub：未配置本地驱动。`probe` 返回不可用。
#[derive(Clone, Copy, Debug, Default)]
pub struct StubLocalDriver;

#[async_trait]
impl LocalDriver for StubLocalDriver {
    async fn act(
        &self,
        _action: BrowserComputerAction,
        _workspace_id: &WorkspaceId,
        _cancel: CancellationToken,
        _authorization: SandboxAuthorization,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        Err(BrowserComputerError::Backend {
            backend: "local",
            message: "no local browser driver is configured".into(),
        })
    }

    async fn snapshot(
        &self,
        _workspace_id: &WorkspaceId,
        _cancel: CancellationToken,
        _authorization: SandboxAuthorization,
    ) -> Result<BrowserComputerSnapshot, BrowserComputerError> {
        Err(BrowserComputerError::Backend {
            backend: "local",
            message: "no local browser driver is configured".into(),
        })
    }
}

/// Local 后端。
#[derive(Clone)]
pub struct LocalBackend {
    driver: Arc<dyn LocalDriver>,
    gate: Arc<Mutex<SandboxGate>>,
    descriptor_name: &'static str,
    available: bool,
    unavailable_reason: String,
}

impl LocalBackend {
    /// 以默认 stub 构造（不可用；in-process，明确不 spawn）。
    pub fn new() -> Self {
        Self::with_driver(Arc::new(StubLocalDriver))
    }

    /// 注入自定义驱动；探测默认可用。默认视为 in-process（不得 spawn）。
    pub fn with_driver(driver: Arc<dyn LocalDriver>) -> Self {
        Self {
            driver,
            gate: Arc::new(Mutex::new(SandboxGate::in_process())),
            descriptor_name: "browser_computer.local",
            available: true,
            unavailable_reason: String::new(),
        }
    }

    /// 声明为 in-process（明确不 spawn 子进程）。
    pub fn in_process(self) -> Self {
        *self.gate.lock().unwrap() = SandboxGate::in_process();
        self
    }

    /// 声明为进程型（driver 需 spawn 浏览器子进程）：必须注入 sandbox，
    /// 未注入时 act/snapshot/spawn fail closed，不降级为 in-process。
    pub fn process_style(self) -> Self {
        *self.gate.lock().unwrap() = SandboxGate::spawn_required();
        self
    }

    /// 注入 Core-owned sandbox：本地浏览器驱动子进程经 sandbox spawn。
    pub fn with_sandbox(self, sandbox: Arc<dyn SandboxBackend>) -> Self {
        *self.gate.lock().unwrap() = SandboxGate::with_sandbox(sandbox);
        self
    }

    /// 当前进程闸门（审计 / 测试检视）。
    pub fn process_gate(&self) -> ProcessMode {
        self.gate.lock().unwrap().mode()
    }

    /// 已注入的 sandbox（供驱动实现 / 测试使用）。
    pub fn sandbox(&self) -> Option<Arc<dyn SandboxBackend>> {
        self.gate.lock().unwrap().sandbox().cloned()
    }

    /// 经注入 sandbox spawn 本地浏览器驱动子进程；未注入 / in-process fail closed。
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

    /// 显式覆盖探测结果（装配层据真实浏览器可用性设置）。
    pub fn with_probe(mut self, available: bool, reason: impl Into<String>) -> Self {
        self.available = available;
        self.unavailable_reason = reason.into();
        self
    }
}

impl Default for LocalBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BrowserComputerBackend for LocalBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Local
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
        if self.available {
            BackendProbe::available()
        } else {
            BackendProbe::unavailable(self.unavailable_reason.clone())
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
                backend: "local",
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
                backend: "local",
                message: err.to_string(),
            }
        })?;
        self.driver
            .snapshot(workspace_id, cancel, authorization)
            .await
    }
}
