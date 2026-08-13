//! Browser / Computer Runtime 的错误类型（P17-10）。
//!
//! 所有变体均可向调用方暴露原因；不携带任何 secret 或大 payload——大输出经
//! [`crate::artifact`] 折叠为 artifact 引用后才进入错误/结果。
use thiserror::Error;

/// Browser / Computer 能力执行错误。
#[derive(Debug, Clone, Error)]
pub enum BrowserComputerError {
    /// 工具入参无法解析为 canonical action。
    #[error("browser/computer action input is invalid: {0}")]
    InvalidInput(String),
    /// Policy 引擎直接拒绝。
    #[error("policy denied browser/computer action: {0}")]
    PolicyDenied(String),
    /// Policy 引擎要求用户审批（本运行未接入审批通道时归一为此错误）。
    #[error("policy requires user approval before action: {0}")]
    PolicyAskUser(String),
    /// 没有任何 ClientFunction 位点的后端可用于本地执行。
    #[error("no browser/computer backend available for local (ClientFunction) execution")]
    NoLocalBackend,
    /// 本地后端不可用，且 Policy 允许跨 trust 降级到 provider-hosted。
    ///
    /// 这是显式、可观测的降级信号：调用方应改走 hosted（ServerToolEvent）路径，
    /// 而不是在本地执行 hosted 后端。
    #[error("local backend unavailable; policy permits cross-trust fallback to provider-hosted (attempted {attempted})")]
    HostedFallbackRequired { attempted: String },
    /// 该后端不允许进入本地 `AgentTool::execute()` 路径。
    ///
    /// ProviderHosted 后端的 `act`/`snapshot` 必须返回此错误；其生命周期走
    /// `ServerToolEvent`，而非本地 execute。
    #[error("backend `{backend}` is not locally executable; its site is {site}")]
    NotLocallyExecutable {
        backend: &'static str,
        site: &'static str,
    },
    /// 后端执行失败。
    #[error("backend `{backend}` failed: {message}")]
    Backend {
        backend: &'static str,
        message: String,
    },
    /// Core-owned 进程闸门拒绝（未注入 sandbox / 违规 spawn 尝试）。
    ///
    /// act/snapshot/spawn 在触达 driver 之前经 `SandboxGate` 因果检查；进程型
    /// 后端未注入 sandbox 时一律 fail closed，不得降级为 in-process 执行。
    #[error("browser/computer sandbox gate denied for `{backend}`: {message}")]
    SandboxDenied {
        backend: &'static str,
        message: String,
    },
    /// 配置了 durable audit sink 时落盘失败（在副作用前 fail-closed）。
    #[error("browser/computer durable audit failed: {0}")]
    AuditSink(String),
    /// 跨 trust boundary 的回退被策略拒绝。
    ///
    /// 隐式跨 trust 切换是被禁止的；只有显式允许时才放行，并附可观测审计记录。
    #[error("cross-trust-boundary fallback denied by policy: attempted {attempted}")]
    CrossTrustFallbackDenied { attempted: String },
    /// 操作被取消。
    #[error("browser/computer operation cancelled")]
    Cancelled,
    /// Artifact 存储失败。
    #[error("artifact store error: {0}")]
    Artifact(String),
}
