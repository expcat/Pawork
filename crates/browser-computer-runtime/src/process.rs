//! Core-owned 进程执行闸门（P17-10 review）。
//!
//! Local / Playwright / 本地 MCP 进程后端统一持有一个 [`SandboxGate`]：
//! - 进程型后端（`SpawnViaSandbox`）的所有操作（act / snapshot / spawn）在触达
//!   driver 之前**因果**经 [`SandboxGate::acquire`] 发放 [`SandboxAuthorization`]：
//!   未注入 `SandboxBackend` 时 acquire fail closed，操作不执行；
//! - in-process / preconnected 后端经 acquire 拿到显式「不 spawn」授权，授权上任何
//!   spawn 尝试都返回明确的 Denied；
//! - 重启 / 恢复不存在降级路径：进程型后端在 sandbox 缺失时一律失败，不会静默
//!   退回 in-process 执行；
//! - 注入由 facade 装配统一完成（`BrowserComputerBackend::inject_sandbox`），
//!   也可在构造期经 `with_sandbox` / `with_preconnected` 显式设置。
//!
//! 约束：Core-owned 子进程绝不绕过 sandbox 直接 spawn（ADR-031）。
use std::sync::Arc;

use agent_domain::CancellationToken;
use sandbox_runtime::{
    SandboxBackend, SandboxError, SandboxPolicy, SandboxProcess, SandboxProcessSpec,
};

/// Core-owned 后端的进程执行模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessMode {
    /// 驱动在 Core 进程内执行（in-process），不得 spawn 子进程。
    InProcess,
    /// 已预先建立连接（如已连接的 MCP transport / CDP），不得 spawn 子进程。
    Preconnected,
    /// 进程型后端：经注入的 `SandboxBackend` spawn，未注入即 fail closed。
    SpawnViaSandbox,
}

impl ProcessMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InProcess => "in_process",
            Self::Preconnected => "preconnected",
            Self::SpawnViaSandbox => "spawn_via_sandbox",
        }
    }
}

/// 一次驱动执行授权：由 [`SandboxGate::acquire`] 发放，driver 的 act/snapshot
/// **必须**接收。
///
/// - 进程型（`SpawnViaSandbox`）：携带已注入的 sandbox 句柄，driver 只能经它
///   spawn；sandbox 缺失时 acquire 直接失败，driver 不可达；
/// - in-process / preconnected：授权显式声明不 spawn，任何 spawn 尝试返回 Denied。
#[derive(Clone)]
pub struct SandboxAuthorization {
    mode: ProcessMode,
    sandbox: Option<Arc<dyn SandboxBackend>>,
}

impl SandboxAuthorization {
    pub fn mode(&self) -> ProcessMode {
        self.mode
    }

    /// 进程型授权携带的 sandbox 句柄（in-process / preconnected 恒为 None）。
    pub fn sandbox(&self) -> Option<&Arc<dyn SandboxBackend>> {
        self.sandbox.as_ref()
    }

    pub fn is_sandboxed(&self) -> bool {
        self.sandbox.is_some()
    }

    /// 授权下的 spawn：in-process / preconnected 显式拒绝；进程型经 sandbox。
    pub async fn spawn(
        &self,
        spec: SandboxProcessSpec,
        policy: SandboxPolicy,
        cancel: CancellationToken,
    ) -> Result<SandboxProcess, SandboxError> {
        match self.mode {
            ProcessMode::InProcess => Err(SandboxError::Denied(
                "in-process backend must not spawn child processes".into(),
            )),
            ProcessMode::Preconnected => Err(SandboxError::Denied(
                "preconnected backend must not spawn child processes".into(),
            )),
            ProcessMode::SpawnViaSandbox => match self.sandbox.as_ref() {
                Some(sandbox) => sandbox.spawn(spec, policy, cancel).await,
                None => Err(SandboxError::Denied(
                    "authorization carries no sandbox; refusing to spawn".into(),
                )),
            },
        }
    }
}

/// Core-owned 子进程的统一 spawn 闸门。
///
/// 三个进程型后端（Local / Playwright / 本地 MCP）共享此闸门，保证
/// 「经注入 sandbox spawn，否则 fail closed；in-process / preconnected
/// 明确不 spawn」的一致性。
#[derive(Clone)]
pub struct SandboxGate {
    mode: ProcessMode,
    sandbox: Option<Arc<dyn SandboxBackend>>,
}

impl SandboxGate {
    /// in-process 闸门：任何 spawn 都显式拒绝。
    pub fn in_process() -> Self {
        Self {
            mode: ProcessMode::InProcess,
            sandbox: None,
        }
    }

    /// preconnected 闸门：任何 spawn 都显式拒绝。
    pub fn preconnected() -> Self {
        Self {
            mode: ProcessMode::Preconnected,
            sandbox: None,
        }
    }

    /// 进程型闸门：必须注入 sandbox；未注入时 spawn fail closed。
    pub fn spawn_required() -> Self {
        Self {
            mode: ProcessMode::SpawnViaSandbox,
            sandbox: None,
        }
    }

    /// 注入 sandbox（进程型后端使用）。
    pub fn with_sandbox(sandbox: Arc<dyn SandboxBackend>) -> Self {
        Self {
            mode: ProcessMode::SpawnViaSandbox,
            sandbox: Some(sandbox),
        }
    }

    pub fn mode(&self) -> ProcessMode {
        self.mode
    }

    /// 已注入的 sandbox（无则为 None）。
    pub fn sandbox(&self) -> Option<&Arc<dyn SandboxBackend>> {
        self.sandbox.as_ref()
    }

    /// 是否已注入 sandbox（审计 / 探测用）。
    pub fn is_sandboxed(&self) -> bool {
        self.sandbox.is_some()
    }

    /// 因果闸门：为一次操作发放执行授权。
    ///
    /// - in-process / preconnected：返回显式「不 spawn」授权（操作可在进程内执行）；
    /// - `SpawnViaSandbox` + 已注入 sandbox：返回携带 sandbox 句柄的授权；
    /// - `SpawnViaSandbox` + 未注入：fail closed（`SandboxError::Denied`），
    ///   操作不得执行。
    pub fn acquire(&self) -> Result<SandboxAuthorization, SandboxError> {
        match self.mode {
            ProcessMode::InProcess | ProcessMode::Preconnected => Ok(SandboxAuthorization {
                mode: self.mode,
                sandbox: None,
            }),
            ProcessMode::SpawnViaSandbox => match self.sandbox.as_ref() {
                Some(sandbox) => Ok(SandboxAuthorization {
                    mode: self.mode,
                    sandbox: Some(sandbox.clone()),
                }),
                None => Err(SandboxError::Denied(
                    "Core-owned process-style backend has no injected sandbox; operation blocked (fail closed)"
                        .into(),
                )),
            },
        }
    }

    /// 统一 spawn 入口。
    pub async fn spawn(
        &self,
        spec: SandboxProcessSpec,
        policy: SandboxPolicy,
        cancel: CancellationToken,
    ) -> Result<SandboxProcess, SandboxError> {
        let authorization = self.acquire()?;
        authorization.spawn(spec, policy, cancel).await
    }
}
