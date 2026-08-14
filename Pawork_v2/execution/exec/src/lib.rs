//! pawork-exec：进程树 + 沙箱。
//!
//! 本 crate 不依赖 pawork-domain / pawork-policy（W1 自含）。
//! 取消令牌用本 crate `cancel`；路径判断用本 crate `path`。

pub mod cancel;
mod os;
mod path;
mod process;
mod sandbox;
mod tree;

pub use cancel::CancellationToken;
pub use process::{
    CommandSpec, ProcessError, ProcessEvent, ProcessHandle, ProcessInput, ProcessLimits,
    ProcessOutput, ProcessRuntime,
};
#[cfg(target_os = "linux")]
pub use process::LinuxLandlockPolicy;
pub use sandbox::{
    default_env_allowlist, default_secret_paths, BackendSelection, FilesystemPolicy, IsolationLevel,
    NativeRestricted, NetworkMode, ProbeOutcome, ResourceLimits, SandboxBackend, SandboxError,
    SandboxInteractiveProcess, SandboxPolicy, SandboxProcess, SandboxProcessSpec, SandboxSelector,
};
pub use tree::ProcessTreeGuard;

pub use os::linux::{
    bwrap_probe_reason, generate_bwrap_argv, probe_landlock_support, LandlockSupport,
};
pub use os::macos::{
    escape_seatbelt_string, generate_seatbelt_profile, probe_reason as sandbox_exec_probe_reason,
    SANDBOX_EXEC_PATH,
};
pub use os::windows::{
    policy_to_appcontainer_config, probe_appcontainer_job, AppContainerCapability,
    AppContainerConfig,
};
