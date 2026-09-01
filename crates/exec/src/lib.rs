//! pawork-exec：进程树 + 沙箱 + PTY。
//!
//! 本 crate 不依赖 pawork-domain / pawork-policy（W1 自含）。
//! 取消令牌用本 crate `cancel`；路径判断用本 crate `path`。
//! PTY 输出留在本模块环形缓冲，不写入 Agent Event Store。

pub mod cancel;
mod os;
mod path;
mod process;
mod pty;
mod sandbox;
mod tree;

pub use cancel::CancellationToken;
#[cfg(target_os = "linux")]
// R0 D21:包外零消费,降为 crate 内保留;R7 沙箱演进将重新消费,届时恢复可见性。
#[allow(unused_imports)]
pub(crate) use process::LinuxLandlockPolicy;
pub use process::{
    CommandSpec, ProcessError, ProcessEvent, ProcessHandle, ProcessInput, ProcessLimits,
    ProcessOutput, ProcessRuntime,
};
pub use pty::{
    OutputCursor, OwnerSessionId, PtyCreateSpec, PtyError, PtyEvent, PtyOutputChunk, PtyService,
    PtySessionState, PtySnapshot, PtyWindowSize, RingBuffer, RingReadError, TerminalId,
    DEFAULT_BUFFER_CAPACITY,
};
pub use sandbox::{
    default_env_allowlist, default_secret_paths, BackendSelection, FilesystemPolicy,
    IsolationLevel, NativeRestricted, NetworkMode, ProbeOutcome, ResourceLimits, SandboxBackend,
    SandboxError, SandboxInteractiveProcess, SandboxPolicy, SandboxProcess, SandboxProcessSpec,
    SandboxSelector,
};
pub use tree::ProcessTreeGuard;

#[allow(unused_imports)]
pub(crate) use os::linux::{
    bwrap_probe_reason, generate_bwrap_argv, probe_landlock_support, LandlockSupport,
};
#[allow(unused_imports)]
pub(crate) use os::macos::{
    escape_seatbelt_string, generate_seatbelt_profile, probe_reason as sandbox_exec_probe_reason,
    SANDBOX_EXEC_PATH,
};
#[allow(unused_imports)]
pub(crate) use os::windows::{
    policy_to_appcontainer_config, probe_appcontainer_job, AppContainerCapability,
    AppContainerConfig,
};
