//! pawork-exec：进程树 + 沙箱 + PTY。
//!
//! 路径判定复用 `pawork-policy`（ADR-052）；不直接依赖 `pawork-domain`。
//! 取消令牌用本 crate `cancel`（与 domain 令牌隔离）。
//! PTY 输出留在本模块环形缓冲，不写入 Agent Event Store。

mod cancel;
mod os;
mod process;
mod pty;
mod sandbox;
mod tree;

pub use cancel::CancellationToken;
pub use process::{
    CommandSpec, ProcessError, ProcessEvent, ProcessHandle, ProcessInput, ProcessLimits,
    ProcessOutput, ProcessRuntime,
};
pub use pty::{
    OutputCursor, OwnerSessionId, PtyCreateSpec, PtyError, PtyEvent, PtyService, PtySessionState,
    PtySnapshot, PtyWindowSize, TerminalId, DEFAULT_BUFFER_CAPACITY,
};
pub use sandbox::{
    default_env_allowlist, default_secret_paths, BackendSelection, FilesystemPolicy,
    IsolationLevel, NativeRestricted, NetworkMode, ProbeOutcome, ResourceLimits, SandboxBackend,
    SandboxError, SandboxInteractiveProcess, SandboxPolicy, SandboxProcess, SandboxProcessSpec,
    SandboxSelector,
};
pub use tree::ProcessTreeGuard;
