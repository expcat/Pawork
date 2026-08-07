//! Sandbox Runtime（P11-1 骨架）。
//!
//! 在 process_runtime 之上叠加执行隔离：把 Agent 调度的子进程约束在
//! workspace 路径、清洗后的环境与有限资源内。设计与完整契约见
//! docs/features/sandbox.md。
//!
//! 本骨架冻结 SandboxBackend trait 与 SandboxPolicy 契约，并提供
//! 永远可用的 NativeRestricted 软沙箱后端；平台原生硬隔离后端
//! （bwrap / sandbox-exec / AppContainer）随 P11-2/3/4 落地，经
//! SandboxSelector 探测与回退接入。

use std::path::PathBuf;

use agent_domain::CancellationToken;
use async_trait::async_trait;
use process_runtime::{CommandSpec, ProcessError, ProcessEvent, ProcessHandle, ProcessRuntime};
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

impl From<&policy_engine::ExecutionConstraints> for ResourceLimits {
    fn from(c: &policy_engine::ExecutionConstraints) -> Self {
        Self {
            wall_time_ms: c.timeout_ms,
            max_output_bytes: c.max_output_bytes,
            ..Default::default()
        }
    }
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
            network_mode: NetworkMode::Hint,
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
    /// 工具声明需要网络（ToolCapability::Network）。
    pub needs_network: bool,
}

/// 受控进程句柄：事件流 + 进程树终止。
pub struct SandboxProcess {
    /// 与 process_runtime::ProcessRuntime::spawn_stream 一致的事件流。
    pub events: mpsc::Receiver<ProcessEvent>,
    handle: ProcessHandle,
}

impl SandboxProcess {
    /// 终止整个进程树（复用 process-runtime 的统一路径）。
    pub async fn kill(&mut self) -> Result<(), SandboxError> {
        self.handle.kill().await.map_err(SandboxError::Process)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("sandbox policy denied: {0}")]
    Denied(String),
    #[error("path outside sandbox roots: {0}")]
    PathEscape(String),
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
        // 1. spawn 许可。
        if !policy.allow_spawn {
            return Err(SandboxError::Denied("spawn disallowed by policy".into()));
        }
        // 2. cwd 锁定到 workspace（若指定 cwd，必须在某 root 内且不在 deny）。
        if let Some(cwd) = spec.command.cwd.clone() {
            ensure_within(&cwd, &spec.workspace_roots, &policy)?;
        }
        // 3. env 清洗：denylist 覆盖 allowlist。
        apply_env(&mut spec.command, &policy);
        // 4. 资源约束：输出上限与墙钟超时。
        if let Some(max) = policy.resources.max_output_bytes {
            spec.command.max_output_bytes = max;
        }
        if let Some(ms) = policy.resources.wall_time_ms {
            spec.command.timeout = Some(std::time::Duration::from_millis(ms));
        }
        // 5. 网络：NativeRestricted 无法硬隔离，network.mode 退化为 Hint（记录由审计承担）。
        // 6. 委托 process-runtime spawn_stream，保证 IO/timeout/cancel/进程树语义一致。
        let (events, handle) = self
            .runtime
            .spawn_stream(spec.command, cancel)
            .await
            .map_err(SandboxError::Process)?;
        Ok(SandboxProcess { events, handle })
    }
}

/// 沙箱后端选择器：按平台探测硬隔离，失败回退 NativeRestricted。
///
/// 本骨架阶段恒返回 NativeRestricted；P11-2/3/4 在 pick 内插入各平台探测槽位
/// （bwrap / sandbox-exec / AppContainer），探测失败则回退，并通过
/// BackendSelection 暴露选择结果供审计/诊断（回退必须可观测）。
#[derive(Clone, Debug, Default)]
pub struct SandboxSelector;

impl SandboxSelector {
    /// 选择当前平台可用的最强后端。
    pub fn pick(&self) -> (NativeRestricted, BackendSelection) {
        let backend = NativeRestricted::new();
        let selection = BackendSelection {
            id: backend.id(),
            fallback: false,
            note: "platform native backends pending P11-2/3/4".into(),
        };
        (backend, selection)
    }
}

/// 后端选择结果（用于审计/诊断）。
#[derive(Clone, Debug)]
pub struct BackendSelection {
    pub id: &'static str,
    /// 是否为探测失败后的回退。
    pub fallback: bool,
    pub note: String,
}

/// 校验路径在工作区根内且不在 deny 列表。
fn ensure_within(
    target: &std::path::Path,
    roots: &[PathBuf],
    policy: &SandboxPolicy,
) -> Result<(), SandboxError> {
    let normalized = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    for d in &policy.filesystem.deny {
        if normalized.starts_with(d) {
            return Err(SandboxError::Denied(format!(
                "path in deny list: {}",
                d.display()
            )));
        }
    }
    let inside = roots.iter().any(|r| normalized.starts_with(r));
    if !inside {
        return Err(SandboxError::PathEscape(normalized.display().to_string()));
    }
    Ok(())
}

/// 按 policy 清洗 CommandSpec 的环境变量：denylist 覆盖 allowlist。
fn apply_env(cmd: &mut CommandSpec, policy: &SandboxPolicy) {
    if policy.env_clear {
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

fn default_secret_paths() -> Vec<PathBuf> {
    // 平台精确密钥路径在 P11-2/3/4 细化；此处给基线。
    #[cfg(unix)]
    if let Some(home) = std::env::var_os("HOME") {
        return vec![PathBuf::from(home).join(".ssh")];
    }
    #[cfg(windows)]
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return vec![PathBuf::from(appdata)];
    }
    Vec::new()
}

fn default_env_allowlist() -> Vec<String> {
    vec![
        "PATH".into(),
        "HOME".into(),
        "LANG".into(),
        "LC_ALL".into(),
        "TERM".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untrusted_default_is_read_only_and_no_spawn() {
        let p = SandboxPolicy::untrusted_default(vec![PathBuf::from("/tmp/pawork-ws")]);
        assert!(p.filesystem.write_roots.is_empty(), "未信任应只读");
        assert!(!p.allow_spawn);
        assert_eq!(p.network_mode, NetworkMode::Hint);
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
    fn execution_constraints_map_to_resource_limits() {
        let c = policy_engine::ExecutionConstraints {
            timeout_ms: Some(30_000),
            max_output_bytes: Some(1_048_576),
        };
        let r = ResourceLimits::from(&c);
        assert_eq!(r.wall_time_ms, Some(30_000));
        assert_eq!(r.max_output_bytes, Some(1_048_576));
    }

    #[test]
    fn selector_picks_native_restricted() {
        let (backend, selection) = SandboxSelector.pick();
        assert_eq!(backend.id(), "native_restricted");
        assert!(backend.available());
        assert_eq!(selection.id, "native_restricted");
        assert!(!selection.fallback);
    }

    #[test]
    fn env_matches_supports_wildcards() {
        assert!(env_matches("*TOKEN*", "GITHUB_TOKEN"));
        assert!(env_matches("AWS*", "AWS_SECRET_KEY"));
        assert!(env_matches("PATH", "PATH"));
        assert!(!env_matches("PATH", "GITHUB_TOKEN"));
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
            needs_network: false,
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
