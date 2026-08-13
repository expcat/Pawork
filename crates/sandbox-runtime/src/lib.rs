//! Sandbox Runtime（Phase 11）。
//!
//! 在 process_runtime 之上叠加执行隔离：把 Agent 调度的子进程约束在
//! workspace 路径、清洗后的环境与有限资源内。设计与完整契约见
//! docs/features/sandbox.md。
//!
//! `NativeRestricted` 永远可用；平台后端经 `SandboxSelector` 探测，
//! 不可用时携带结构化原因回退，不会把软限制伪装成硬隔离。

use std::path::PathBuf;

use agent_domain::CancellationToken;
use async_trait::async_trait;
use process_runtime::{
    CommandSpec, ProcessError, ProcessEvent, ProcessHandle, ProcessInput, ProcessLimits,
    ProcessRuntime,
};

mod backends;

// 平台后端的纯函数配置生成器对外可见：供审计/诊断展示与 L0 三平台单测。
pub use backends::linux::{
    bwrap_probe_reason, generate_bwrap_argv, probe_landlock_support, LandlockSupport,
};
pub use backends::macos::{
    escape_seatbelt_string, generate_seatbelt_profile, probe_reason as sandbox_exec_probe_reason,
    SANDBOX_EXEC_PATH,
};
pub use backends::windows::{
    policy_to_appcontainer_config, probe_appcontainer_job, AppContainerCapability,
    AppContainerConfig,
};

use tokio::sync::mpsc;

/// 网络策略模式。
///
/// Enforce 需要平台原生硬隔离后端才能保证；NativeRestricted
/// 仅能提供 Hint（记录但不强制）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    /// 不施加网络约束。
    Off,
    /// 仅记录出站意图，不强制（NativeRestricted 的固有降级）。
    Hint,
    /// 强制阻断未授权出站（硬隔离后端）。
    #[default]
    Enforce,
}

/// 文件系统策略。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct FilesystemPolicy {
    /// 允许读取的根（只读）。
    pub read_roots: Vec<PathBuf>,
    /// 允许写入的根。
    pub write_roots: Vec<PathBuf>,
    /// 显式拒绝（如密钥目录），优先级最高。
    pub deny: Vec<PathBuf>,
}

/// 资源限制。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ResourceLimits {
    pub cpu_seconds: Option<u64>,
    pub memory_mb: Option<u64>,
    pub open_fds: Option<u64>,
    pub wall_time_ms: Option<u64>,
    pub max_output_bytes: Option<u64>,
}

/// 沙箱策略：声明式，后端据此构造平台原生约束。
///
/// 最终语义以后端实际能力为准（见 docs/features/sandbox.md）。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SandboxPolicy {
    pub filesystem: FilesystemPolicy,
    pub network_mode: NetworkMode,
    pub network_allow_hosts: Vec<String>,
    pub allow_spawn: bool,
    pub max_procs: Option<u32>,
    pub env_clear: bool,
    pub env_allowlist: Vec<String>,
    pub env_denylist: Vec<String>,
    pub resources: ResourceLimits,
}

impl SandboxPolicy {
    /// 未信任工作区的最小权限默认策略：仅 workspace 只读、禁 spawn、
    /// env 清洗、Secret 目录拒绝、网络仅 Hint。
    pub fn untrusted_default(workspace_roots: Vec<PathBuf>) -> Self {
        Self {
            filesystem: FilesystemPolicy {
                read_roots: workspace_roots,
                write_roots: Vec::new(),
                deny: default_secret_paths(),
            },
            network_mode: NetworkMode::Enforce,
            allow_spawn: false,
            env_clear: true,
            env_allowlist: default_env_allowlist(),
            ..Default::default()
        }
    }
}

/// 沙箱进程规格：在 CommandSpec 上增加沙箱注解。
#[derive(Clone, Debug)]
pub struct SandboxProcessSpec {
    pub command: CommandSpec,
    pub workspace_roots: Vec<PathBuf>,
}

/// 受控进程：事件流 + 进程树生命周期。
pub struct SandboxProcess {
    /// 与 process_runtime::ProcessRuntime::spawn_stream 一致的事件流。
    pub events: mpsc::Receiver<ProcessEvent>,
    /// 进程树生命周期守卫：`ProcessHandle::drop` 会取消 kill token 并终止整棵进程树，
    /// 因此必须与 `events` 同生命周期（§2.6(b)：`kill` 方法已删，字段保留不可移除）。
    #[allow(dead_code)]
    _handle: ProcessHandle,
}

/// 可双向通信的受控进程；用于 LSP/MCP 等 stdio 协议。
///
/// stdin、输出事件与生命周期来自同一次 Sandbox → Process Runtime spawn。
pub struct SandboxInteractiveProcess {
    pub events: mpsc::Receiver<ProcessEvent>,
    pub input: ProcessInput,
    handle: ProcessHandle,
}

impl SandboxInteractiveProcess {
    /// 拆分读、写、生命周期三部分，供协议适配器独立持有。
    pub fn into_parts(self) -> (mpsc::Receiver<ProcessEvent>, ProcessInput, ProcessHandle) {
        (self.events, self.input, self.handle)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("sandbox policy denied: {0}")]
    Denied(String),
    #[error("path outside sandbox roots: {0}")]
    PathEscape(String),
    #[error("sandbox backend unavailable on this system: {0}")]
    BackendUnavailable(&'static str),
    #[error(transparent)]
    Process(#[from] ProcessError),
}

/// 沙箱后端 trait：所有平台后端实现同一接口，调用方只感知 policy -> spawn。
#[async_trait]
pub trait SandboxBackend: Send + Sync {
    /// 后端标识（如 "native_restricted"、"bwrap"、"sandbox_exec"、"appcontainer"）。
    fn id(&self) -> &'static str;
    /// 该后端在当前系统是否可用（探测结果，不应在 spawn 热路径上重算）。
    fn available(&self) -> bool;
    /// 按策略 spawn 一个受控进程。
    async fn spawn(
        &self,
        spec: SandboxProcessSpec,
        policy: SandboxPolicy,
        cancel: CancellationToken,
    ) -> Result<SandboxProcess, SandboxError>;

    /// 按相同沙箱策略启动带 stdin 的双向受控进程。
    ///
    /// 默认显式拒绝，避免第三方后端静默降级为裸进程。
    async fn spawn_interactive(
        &self,
        _spec: SandboxProcessSpec,
        _policy: SandboxPolicy,
        _cancel: CancellationToken,
    ) -> Result<SandboxInteractiveProcess, SandboxError> {
        Err(SandboxError::BackendUnavailable(
            "interactive process I/O is not implemented by this sandbox backend",
        ))
    }
}

/// NativeRestricted：纯 Rust 软沙箱，永远可用。
///
/// 提供纵深防御第一层：env 清洗、cwd 锁定、Secret 目录拒绝、资源/输出上限。
/// 它不是对抗性隔离边界——挡不住已授权命令内部的越权行为（如
/// sh -c "cat ~/.ssh/id_rsa"）；硬隔离见 P11-2/3/4。
#[derive(Clone, Debug, Default)]
pub struct NativeRestricted {
    runtime: ProcessRuntime,
}

impl NativeRestricted {
    pub fn new() -> Self {
        Self {
            runtime: ProcessRuntime::new(),
        }
    }

    pub fn with_runtime(runtime: ProcessRuntime) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl SandboxBackend for NativeRestricted {
    fn id(&self) -> &'static str {
        "native_restricted"
    }

    fn available(&self) -> bool {
        true
    }

    async fn spawn(
        &self,
        mut spec: SandboxProcessSpec,
        policy: SandboxPolicy,
        cancel: CancellationToken,
    ) -> Result<SandboxProcess, SandboxError> {
        // 1. 软限制第一层（spawn 许可 / cwd 锁定 / env 清洗 / 资源上限），所有后端共用。
        apply_soft_restrictions(&mut spec, &policy)?;
        // 2. 网络：NativeRestricted 无法硬隔离。当策略要求 Enforce 时显式降级为 Hint，
        //    并经 tracing 可观测——绝不静默声称已强制。
        if policy.network_mode == NetworkMode::Enforce {
            tracing::warn!(
                target: "pawork.sandbox",
                backend = "native_restricted",
                network_mode = ?policy.network_mode,
                "network_mode=Enforce not enforceable by NativeRestricted; degraded to Hint (audit only, no hard isolation)"
            );
        }
        // 3. 委托 process-runtime spawn_stream，保证 IO/timeout/cancel/进程树语义一致。
        let (events, handle) = self
            .runtime
            .spawn_stream(spec.command, cancel)
            .await
            .map_err(SandboxError::Process)?;
        Ok(SandboxProcess {
            events,
            _handle: handle,
        })
    }

    async fn spawn_interactive(
        &self,
        mut spec: SandboxProcessSpec,
        policy: SandboxPolicy,
        cancel: CancellationToken,
    ) -> Result<SandboxInteractiveProcess, SandboxError> {
        apply_soft_restrictions(&mut spec, &policy)?;
        if policy.network_mode == NetworkMode::Enforce {
            tracing::warn!(
                target: "pawork.sandbox",
                backend = "native_restricted",
                network_mode = ?policy.network_mode,
                "network_mode=Enforce not enforceable by NativeRestricted; degraded to Hint (audit only, no hard isolation)"
            );
        }
        let (events, input, handle) = self
            .runtime
            .spawn_interactive(spec.command, cancel)
            .await
            .map_err(SandboxError::Process)?;
        Ok(SandboxInteractiveProcess {
            events,
            input,
            handle,
        })
    }
}

/// 沙箱后端选择器：按平台探测硬隔离，失败回退 NativeRestricted。
///
/// `pick` 按平台依次探测硬隔离后端（macOS sandbox-exec / Linux bwrap→Landlock /
/// Windows AppContainer→Job），任一可用即返回；全部不可用则回退 NativeRestricted。
/// 所有探测尝试与回退都写入 [`BackendSelection`]，保证降级可观测、不静默。
#[derive(Clone, Debug, Default)]
pub struct SandboxSelector {
    runtime: ProcessRuntime,
}

impl SandboxSelector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_runtime(runtime: ProcessRuntime) -> Self {
        Self { runtime }
    }

    /// 选择当前平台可用的最强后端。
    pub fn pick(&self) -> (Box<dyn SandboxBackend>, BackendSelection) {
        let mut attempted: Vec<ProbeOutcome> = Vec::new();

        #[cfg(target_os = "macos")]
        {
            let backend = backends::macos::SandboxExecBackend::with_runtime(self.runtime);
            let avail = backend.available();
            attempted.push(ProbeOutcome {
                backend: backend.id(),
                available: avail,
                reason: backends::macos::probe_reason(),
            });
            if avail {
                let id = backend.id();
                return (
                    Box::new(backend),
                    BackendSelection {
                        id,
                        isolation: IsolationLevel::Hard,
                        fallback: false,
                        note: "macOS sandbox-exec (Seatbelt) hard isolation".into(),
                        attempted,
                    },
                );
            }
        }

        #[cfg(target_os = "linux")]
        {
            let backend = backends::linux::BwrapBackend::with_runtime(self.runtime);
            let avail = backend.available();
            attempted.push(ProbeOutcome {
                backend: backend.id(),
                available: avail,
                reason: backends::linux::bwrap_probe_reason(),
            });
            if avail {
                let id = backend.id();
                return (
                    Box::new(backend),
                    BackendSelection {
                        id,
                        isolation: IsolationLevel::Hard,
                        fallback: false,
                        note: "Linux bwrap hard isolation".into(),
                        attempted,
                    },
                );
            }
            let landlock = backends::linux::probe_landlock_support();
            attempted.push(ProbeOutcome {
                backend: "landlock",
                available: landlock.supported,
                reason: landlock.reason.clone(),
            });
            if landlock.supported {
                let backend = backends::linux::LandlockBackend::with_runtime(self.runtime);
                return (
                    Box::new(backend),
                    BackendSelection {
                        id: "landlock",
                        isolation: IsolationLevel::HardFilesystemOnly,
                        fallback: true,
                        note: "Linux Landlock filesystem isolation; bwrap unavailable, network isolation is not enforced".into(),
                        attempted,
                    },
                );
            }
        }

        #[cfg(windows)]
        {
            // AppContainer 不可承载时仍使用 process-runtime 的 Job Object，
            // 真实施加进程数/内存/CPU限额和 KILL_ON_JOB_CLOSE；文件/网络保持软限制。
            attempted.push(backends::windows::probe_appcontainer_job());
            let backend = backends::windows::WindowsJobBackend::with_runtime(self.runtime);
            attempted.push(ProbeOutcome {
                backend: backend.id(),
                available: backend.available(),
                reason: "Job Object available; AppContainer token unavailable, filesystem/network isolation degraded".into(),
            });
            (
                Box::new(backend),
                BackendSelection {
                    id: "windows_job",
                    isolation: IsolationLevel::Degraded,
                    fallback: true,
                    note: "Windows Job Object process/resource isolation; AppContainer unavailable, filesystem/network restrictions are soft".into(),
                    attempted,
                },
            )
        }

        #[cfg(not(windows))]
        {
            // 回退 NativeRestricted（永远可用）。
            let backend = NativeRestricted::with_runtime(self.runtime);
            let id = backend.id();
            let fallback = !attempted.is_empty();
            let note = if fallback {
                "no usable hard-isolation backend; fell back to NativeRestricted soft sandbox"
                    .into()
            } else {
                "NativeRestricted soft sandbox (no hard-isolation backend probed on this platform)"
                    .into()
            };
            (
                Box::new(backend),
                BackendSelection {
                    id,
                    isolation: IsolationLevel::Soft,
                    fallback,
                    note,
                    attempted,
                },
            )
        }
    }
}

/// 后端选择结果（用于审计/诊断）。回退必须可观测：`attempted` 记录按优先级尝试的
/// 全部后端及其结果，`isolation` 说明实际生效的隔离强度。
#[derive(Clone, Debug, serde::Serialize)]
pub struct BackendSelection {
    pub id: &'static str,
    /// 是否为探测失败后的回退。
    pub fallback: bool,
    pub note: String,
    /// 实际生效的隔离强度。
    pub isolation: IsolationLevel,
    /// 本次选择按优先级尝试过的全部后端及其探测结果（含最终选中者）。
    pub attempted: Vec<ProbeOutcome>,
}

/// 实际生效的隔离强度，由 [`BackendSelection`] 携带，供审计与调用方决策。
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationLevel {
    /// NativeRestricted：env/路径/资源软限制，非对抗性边界。
    Soft,
    /// 平台原生硬隔离（sandbox-exec / bwrap）：文件/网络/进程系统级隔离。
    Hard,
    /// 仅文件系统硬隔离（如 landlock），网络未强制。
    HardFilesystemOnly,
    /// 探测到硬隔离能力但 spawn 路径暂不可用（process-runtime 边界），实际走软沙箱。
    Degraded,
}

impl IsolationLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Soft => "soft",
            Self::Hard => "hard",
            Self::HardFilesystemOnly => "hard_filesystem_only",
            Self::Degraded => "degraded",
        }
    }
}

/// 单个后端的探测结果，是可观测回退的依据。
#[derive(Clone, Debug, serde::Serialize)]
pub struct ProbeOutcome {
    pub backend: &'static str,
    pub available: bool,
    /// 不可用或降级的原因；`available == true` 时可为空。
    pub reason: String,
}

/// 软限制第一层（所有后端共用）：spawn 许可、cwd 锁定、env 清洗、资源上限。
///
/// 硬隔离后端在调用本函数后，再叠加平台原生约束（sandbox-exec profile / bwrap argv）。
pub(crate) fn apply_soft_restrictions(
    spec: &mut SandboxProcessSpec,
    policy: &SandboxPolicy,
) -> Result<(), SandboxError> {
    if !policy.allow_spawn {
        return Err(SandboxError::Denied("spawn disallowed by policy".into()));
    }
    if let Some(cwd) = spec.command.cwd.clone() {
        ensure_within(&cwd, &spec.workspace_roots, policy)?;
    }
    apply_env(&mut spec.command, policy);
    if let Some(max) = policy.resources.max_output_bytes {
        spec.command.max_output_bytes = max;
    }
    if let Some(ms) = policy.resources.wall_time_ms {
        spec.command.timeout = Some(std::time::Duration::from_millis(ms));
    }
    spec.command.limits = ProcessLimits {
        cpu_time: policy
            .resources
            .cpu_seconds
            .map(std::time::Duration::from_secs),
        memory_bytes: policy
            .resources
            .memory_mb
            .map(|mb| mb.saturating_mul(1024 * 1024)),
        open_files: policy.resources.open_fds,
        max_processes: policy.max_procs,
    };
    Ok(())
}

/// 校验路径在工作区根内且不在 deny 列表。
fn ensure_within(
    target: &std::path::Path,
    roots: &[PathBuf],
    policy: &SandboxPolicy,
) -> Result<(), SandboxError> {
    let normalized = policy_engine::canonicalize_platform(target)
        .map_err(|error| SandboxError::PathEscape(format!("{}: {error}", target.display())))?;
    for d in &policy.filesystem.deny {
        let denied = policy_engine::canonicalize_platform(d).unwrap_or_else(|_| d.clone());
        if policy_engine::path_within_root(&normalized, &denied) {
            return Err(SandboxError::Denied(format!(
                "path in deny list: {}",
                d.display()
            )));
        }
    }
    let inside = roots.iter().any(|root| {
        policy_engine::canonicalize_platform(root)
            .map(|root| policy_engine::path_within_root(&normalized, &root))
            .unwrap_or(false)
    });
    if !inside {
        return Err(SandboxError::PathEscape(normalized.display().to_string()));
    }

    let policy_roots = policy
        .filesystem
        .read_roots
        .iter()
        .chain(&policy.filesystem.write_roots);
    let policy_allows = policy_roots
        .map(|root| policy_engine::canonicalize_platform(root))
        .any(|root| {
            root.map(|root| policy_engine::path_within_root(&normalized, &root))
                .unwrap_or(false)
        });
    if !policy_allows {
        return Err(SandboxError::Denied(format!(
            "cwd is not allowed by filesystem policy: {}",
            normalized.display()
        )));
    }
    Ok(())
}

/// 按 policy 清洗 CommandSpec 的环境变量：denylist 覆盖 allowlist。
fn apply_env(cmd: &mut CommandSpec, policy: &SandboxPolicy) {
    // 一旦启用 allow/deny 过滤就必须清空父环境；否则 denylist 无法删除继承变量。
    if policy.env_clear || !policy.env_allowlist.is_empty() || !policy.env_denylist.is_empty() {
        cmd.env_clear = true;
    }
    cmd.env.retain(|(k, _)| {
        if policy.env_denylist.iter().any(|pat| env_matches(pat, k)) {
            return false;
        }
        if policy.env_allowlist.is_empty() {
            return true;
        }
        policy.env_allowlist.iter().any(|pat| env_matches(pat, k))
    });
}

/// 环境变量名匹配：两端 * 表示包含、首部 * 表示后缀、尾部 * 表示前缀，大小写不敏感。
fn env_matches(pattern: &str, name: &str) -> bool {
    let upper_name = name.to_ascii_uppercase();
    let core = pattern.trim_matches('*');
    let starts = pattern.starts_with('*');
    let ends = pattern.ends_with('*');
    let upper_core = core.to_ascii_uppercase();
    match (starts, ends) {
        (true, true) => upper_name.contains(&upper_core),
        (true, false) => upper_name.ends_with(&upper_core),
        (false, true) => upper_name.starts_with(&upper_core),
        (false, false) => upper_name == upper_core,
    }
}

/// 平台密钥/凭据目录拒绝清单（权威单一来源，`untrusted_default` 与
/// builtin-tools 的 run_command 共用；平台精确路径在 P11-2/3/4 细化）。
pub fn default_secret_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        let home = PathBuf::from(home);
        paths.extend([".ssh", ".aws", ".azure", ".kube"].map(|name| home.join(name)));
    }
    #[cfg(windows)]
    if let Some(appdata) = std::env::var_os("APPDATA") {
        paths.push(PathBuf::from(appdata).join("gcloud"));
    }
    paths
}

/// 环境变量透传白名单（权威单一来源，`untrusted_default` 与 builtin-tools 的
/// run_command 共用；unix/Windows 历史平台清单的并集，多出的条目在不存在时自然不生效）。
pub fn default_env_allowlist() -> Vec<String> {
    vec![
        "PATH".into(),
        "HOME".into(),
        "LANG".into(),
        "LC_ALL".into(),
        "TERM".into(),
        "TMPDIR".into(),
        "SYSTEMROOT".into(),
        "TEMP".into(),
        "TMP".into(),
        "USERPROFILE".into(),
        "COMSPEC".into(),
        "PATHEXT".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_PATH_TEST: AtomicU64 = AtomicU64::new(1);

    fn temp_path_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "pawork-sandbox-{}-{}-{name}",
            std::process::id(),
            NEXT_PATH_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).expect("create temp root");
        root
    }

    #[test]
    fn untrusted_default_is_read_only_and_no_spawn() {
        let p = SandboxPolicy::untrusted_default(vec![PathBuf::from("/tmp/pawork-ws")]);
        assert!(p.filesystem.write_roots.is_empty(), "未信任应只读");
        assert!(!p.allow_spawn);
        assert_eq!(p.network_mode, NetworkMode::Enforce);
        assert!(p.env_clear);
        assert!(!p.env_allowlist.is_empty());
    }

    #[test]
    fn env_denylist_filters_secrets_over_allowlist() {
        let mut cmd = CommandSpec::new("sh");
        cmd.env_clear = false;
        cmd.env.push(("PATH".into(), "/usr/bin".into()));
        cmd.env.push(("GITHUB_TOKEN".into(), "secret".into()));
        cmd.env.push(("AWS_SECRET_KEY".into(), "secret".into()));

        let policy = SandboxPolicy {
            env_clear: false,
            env_allowlist: vec!["PATH".into()],
            env_denylist: vec!["*TOKEN*".into(), "*KEY*".into(), "*SECRET*".into()],
            ..Default::default()
        };
        apply_env(&mut cmd, &policy);

        assert!(cmd.env.iter().any(|(k, _)| k == "PATH"));
        assert!(!cmd.env.iter().any(|(k, _)| k == "GITHUB_TOKEN"));
        assert!(!cmd.env.iter().any(|(k, _)| k == "AWS_SECRET_KEY"));
    }

    #[test]
    fn selector_pick_returns_runnable_and_observable_backend() {
        let (backend, selection) = SandboxSelector::new().pick();
        // 选中的后端与 selection.id 一致，且真的可用（可立即 spawn）。
        assert_eq!(backend.id(), selection.id);
        assert!(
            backend.available(),
            "picked backend must be available to spawn"
        );
        // 所有探测尝试都被记录；不可用者必须带原因（可观测回退）。
        for outcome in &selection.attempted {
            assert!(!outcome.backend.is_empty());
            if !outcome.available {
                assert!(
                    !outcome.reason.is_empty(),
                    "unavailable backend must record a reason: {:?}",
                    outcome.backend
                );
            }
        }
    }

    #[test]
    fn fallback_reports_actual_isolation_level() {
        let (_backend, selection) = SandboxSelector::new().pick();
        if selection.fallback {
            match selection.id {
                "landlock" => {
                    assert_eq!(selection.isolation, IsolationLevel::HardFilesystemOnly)
                }
                "native_restricted" => assert_eq!(selection.isolation, IsolationLevel::Soft),
                "windows_job" => assert_eq!(selection.isolation, IsolationLevel::Degraded),
                other => panic!("unexpected fallback backend: {other}"),
            }
            assert!(
                selection.attempted.iter().any(|o| !o.available),
                "fallback must be backed by at least one failed probe"
            );
        }
    }

    #[test]
    fn env_matches_supports_wildcards() {
        assert!(env_matches("*TOKEN*", "GITHUB_TOKEN"));
        assert!(env_matches("AWS*", "AWS_SECRET_KEY"));
        assert!(env_matches("PATH", "PATH"));
        assert!(!env_matches("PATH", "GITHUB_TOKEN"));
    }

    #[test]
    fn soft_restrictions_map_os_resource_limits() {
        let mut spec = SandboxProcessSpec {
            command: CommandSpec::new("noop"),
            workspace_roots: Vec::new(),
        };
        let policy = SandboxPolicy {
            allow_spawn: true,
            max_procs: Some(7),
            resources: ResourceLimits {
                cpu_seconds: Some(3),
                memory_mb: Some(64),
                open_fds: Some(32),
                ..Default::default()
            },
            ..Default::default()
        };
        apply_soft_restrictions(&mut spec, &policy).expect("apply limits");
        assert_eq!(
            spec.command.limits.cpu_time,
            Some(std::time::Duration::from_secs(3))
        );
        assert_eq!(spec.command.limits.memory_bytes, Some(64 * 1024 * 1024));
        assert_eq!(spec.command.limits.open_files, Some(32));
        assert_eq!(spec.command.limits.max_processes, Some(7));
    }

    #[test]
    fn cwd_must_be_inside_workspace_and_policy_roots() {
        let root = temp_path_root("root");
        let sibling = temp_path_root("root-sibling");
        let mut spec = SandboxProcessSpec {
            command: CommandSpec::new("noop"),
            workspace_roots: vec![root.clone()],
        };
        spec.command.cwd = Some(sibling.clone());
        let policy = SandboxPolicy {
            filesystem: FilesystemPolicy {
                read_roots: vec![root.clone()],
                ..Default::default()
            },
            allow_spawn: true,
            ..Default::default()
        };
        let error = apply_soft_restrictions(&mut spec, &policy).expect_err("sibling escape");
        assert!(matches!(error, SandboxError::PathEscape(_)));
        std::fs::remove_dir_all(root).expect("cleanup root");
        std::fs::remove_dir_all(sibling).expect("cleanup sibling");
    }

    #[test]
    fn explicit_deny_overrides_allowed_workspace_root() {
        let root = temp_path_root("deny-root");
        let denied = root.join("secret");
        std::fs::create_dir_all(&denied).expect("create denied");
        let mut spec = SandboxProcessSpec {
            command: CommandSpec::new("noop"),
            workspace_roots: vec![root.clone()],
        };
        spec.command.cwd = Some(denied.clone());
        let policy = SandboxPolicy {
            filesystem: FilesystemPolicy {
                read_roots: vec![root.clone()],
                deny: vec![denied],
                ..Default::default()
            },
            allow_spawn: true,
            ..Default::default()
        };
        let error = apply_soft_restrictions(&mut spec, &policy).expect_err("deny wins");
        assert!(matches!(error, SandboxError::Denied(_)));
        std::fs::remove_dir_all(root).expect("cleanup root");
    }

    /// 端到端：真实 spawn 一个子进程打印 secret，验证 apply_env 在
    /// spawn 热路径上确实生效（而非仅在纯函数层）。
    #[tokio::test]
    async fn native_restricted_strips_secret_from_spawned_child() {
        let (program, args) = shell_for_test();
        let mut command = CommandSpec::new(program).args(args);
        // 由 policy 接管清洗，不在此预设 env_clear。
        command
            .env
            .push(("PAWORK_TEST_SECRET".to_string(), "leak-canary".to_string()));

        let backend = NativeRestricted::new();
        let policy = SandboxPolicy {
            allow_spawn: true,
            // 清空父环境，仅按 cmd.env 放回；denylist 会把 secret 剔除。
            env_clear: true,
            env_denylist: vec!["*SECRET*".to_string()],
            ..Default::default()
        };
        let spec = SandboxProcessSpec {
            command,
            workspace_roots: Vec::new(),
        };
        let mut proc = backend
            .spawn(spec, policy, CancellationToken::new())
            .await
            .expect("spawn");

        let mut out = Vec::new();
        while let Some(ev) = proc.events.recv().await {
            match ev {
                ProcessEvent::Stdout(b) => out.extend_from_slice(&b),
                ProcessEvent::Stderr(_) => {}
                ProcessEvent::Exit { .. } => break,
            }
        }
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("MARKER"), "应包含 MARKER 输出: {text}");
        assert!(
            !text.contains("leak-canary"),
            "secret 未被沙箱剔除，发生泄漏: {text}"
        );
    }

    /// 单一来源清单必须是历史平台清单并集的超集（§2.2 防漂移回归）。
    #[test]
    fn default_allowlists_are_authoritative_supersets() {
        let env = default_env_allowlist();
        for name in [
            "PATH",
            "HOME",
            "LANG",
            "LC_ALL",
            "TERM",
            "TMPDIR",
            "SYSTEMROOT",
            "TEMP",
            "TMP",
            "USERPROFILE",
            "COMSPEC",
            "PATHEXT",
        ] {
            assert!(
                env.iter().any(|item| item == name),
                "env allowlist 缺少 {name}"
            );
        }

        let secrets = default_secret_paths();
        if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
            for name in [".ssh", ".aws", ".azure", ".kube"] {
                let expected = PathBuf::from(&home).join(name);
                assert!(
                    secrets.iter().any(|p| p == &expected),
                    "secret paths 缺少 {}",
                    expected.display()
                );
            }
        }
        #[cfg(windows)]
        if let Some(appdata) = std::env::var_os("APPDATA") {
            assert!(secrets
                .iter()
                .any(|p| p == &PathBuf::from(&appdata).join("gcloud")));
        }
    }

    /// 平台 shell 打印 secret 的 argv。
    #[cfg(not(windows))]
    fn shell_for_test() -> (&'static str, Vec<String>) {
        (
            "sh",
            vec![
                "-c".to_string(),
                "echo MARKER=$PAWORK_TEST_SECRET".to_string(),
            ],
        )
    }

    #[cfg(windows)]
    fn shell_for_test() -> (&'static str, Vec<String>) {
        (
            "cmd",
            vec![
                "/d".to_string(),
                "/c".to_string(),
                "echo MARKER=%PAWORK_TEST_SECRET%".to_string(),
            ],
        )
    }
}
