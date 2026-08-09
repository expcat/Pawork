//! Windows AppContainer 配置/探测与 Job Object 执行后端。
//!
//! 配置纯函数 [`policy_to_appcontainer_config`] 跨平台编译并单测；
//! [`probe_appcontainer_job`] 在 Windows 经 kernel32 `IsProcessInJob` 真实探测
//! 当前进程是否已身处 Job（影响 Job 嵌套），非 Windows 返回不可用 stub。
//!
//! AppContainer 受限令牌 spawn 仍需要 `EXTENDED_STARTUPINFO_PRESENT`，因此探测结果
//! 明确标记不可用；可执行路径使用 process-runtime 已实现的 Job Object（资源限额、
//! 活动进程数、`KILL_ON_JOB_CLOSE`），并诚实标记文件/网络隔离为降级。

use crate::{NetworkMode, ProbeOutcome, SandboxPolicy};
use std::sync::OnceLock;

/// AppContainer capability（最小权限集；默认不授予 Internet 以实现网络隔离）。
// frozen, awaiting P11-4.E1: AppContainer restricted-token spawn 尚未接入，
// 生成器仅保留供诊断/审计与 L0 单测，不做任何 spawn 承诺。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppContainerCapability {
    InternetClient,
    InternetClientServer,
    PrivateNetworkClientServer,
    DocumentsLibrary,
    PicturesLibrary,
    VideosLibrary,
    MusicLibrary,
}

/// AppContainer 配置（纯数据，供后续 spawn 后端消费）。
// frozen, awaiting P11-4.E1: 无 spawn 消费方，保留至 AppContainer 后端接入。
#[derive(Clone, Debug, Default)]
pub struct AppContainerConfig {
    pub capabilities: Vec<AppContainerCapability>,
    /// 是否授予 Internet（默认 false = 网络隔离）。
    pub internet_granted: bool,
    pub read_paths: Vec<std::path::PathBuf>,
    pub write_paths: Vec<std::path::PathBuf>,
    pub denied_paths: Vec<std::path::PathBuf>,
}

/// 从 [`SandboxPolicy`] 生成 AppContainer 配置。
///
/// 网络语义：`Enforce` → 不授予 Internet（出站隔离）；`Off`/`Hint` → 授予 Internet
/// （AppContainer 作为硬隔离后端，仅在 `Enforce` 时强制网络隔离）。
// frozen, awaiting P11-4.E1: AppContainer spawn 后端未接入，生成器保留不删。
pub fn policy_to_appcontainer_config(policy: &SandboxPolicy) -> AppContainerConfig {
    let internet_granted = !matches!(policy.network_mode, NetworkMode::Enforce);
    let mut capabilities = Vec::new();
    if internet_granted {
        capabilities.push(AppContainerCapability::InternetClient);
    }
    AppContainerConfig {
        capabilities,
        internet_granted,
        read_paths: policy.filesystem.read_roots.clone(),
        write_paths: policy.filesystem.write_roots.clone(),
        denied_paths: policy.filesystem.deny.clone(),
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn IsProcessInJob(process_handle: isize, job_handle: isize, result: *mut i32) -> i32;
}

#[cfg(windows)]
unsafe fn current_process_in_job() -> bool {
    let mut result: i32 = 0;
    // GetCurrentProcess() 伪句柄 == (HANDLE)-1；job_handle=NULL 探测当前进程所属 Job。
    // SAFETY: 句柄为当前进程伪句柄，结果指针指向有效 i32。
    let ok = unsafe { IsProcessInJob(-1isize, 0isize, &mut result) };
    ok != 0 && result != 0
}

/// 探测 AppContainer 能力。Windows 下经 `IsProcessInJob` 记录父 Job 状态；受限令牌
/// spawn 尚不可用，故 `available` 为 false。调用方随后选择可执行的 Job-only 后端。
pub fn probe_appcontainer_job() -> ProbeOutcome {
    static PROBE: OnceLock<ProbeOutcome> = OnceLock::new();
    PROBE
        .get_or_init(|| {
            #[cfg(windows)]
            {
                let in_job = unsafe { current_process_in_job() };
                ProbeOutcome {
                    backend: "appcontainer_job",
                    available: false,
                    reason: format!(
                        "AppContainer API present (current process in job: {in_job}); restricted-token spawn requires EXTENDED_STARTUPINFO, falling back to Job Object-only isolation"
                    ),
                }
            }
            #[cfg(not(windows))]
            {
                ProbeOutcome {
                    backend: "appcontainer_job",
                    available: false,
                    reason: "AppContainer/Job only available on Windows".to_string(),
                }
            }
        })
        .clone()
}

#[cfg(windows)]
mod job {
    use crate::{
        apply_soft_restrictions, NetworkMode, SandboxBackend, SandboxError, SandboxPolicy,
        SandboxProcess, SandboxProcessSpec,
    };
    use agent_domain::CancellationToken;
    use async_trait::async_trait;
    use process_runtime::ProcessRuntime;

    /// Windows 可执行兜底：process-runtime 会为每个子进程建立 Job Object，施加
    /// 资源/活动进程限制，并通过 KILL_ON_JOB_CLOSE 保证整树生命周期。
    #[derive(Clone, Debug)]
    pub struct WindowsJobBackend {
        runtime: ProcessRuntime,
    }

    impl WindowsJobBackend {
        pub fn new() -> Self {
            Self::with_runtime(ProcessRuntime::new())
        }

        pub fn with_runtime(runtime: ProcessRuntime) -> Self {
            Self { runtime }
        }
    }

    impl Default for WindowsJobBackend {
        fn default() -> Self {
            Self::new()
        }
    }

    #[async_trait]
    impl SandboxBackend for WindowsJobBackend {
        fn id(&self) -> &'static str {
            "windows_job"
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
            apply_soft_restrictions(&mut spec, &policy)?;
            if policy.network_mode == NetworkMode::Enforce {
                tracing::warn!(
                    target: "pawork.sandbox",
                    backend = "windows_job",
                    "AppContainer unavailable; Job Object enforces process/resource limits but not filesystem/network isolation"
                );
            }
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
    }
}

#[cfg(windows)]
pub use job::WindowsJobBackend;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FilesystemPolicy;
    use std::path::PathBuf;

    #[test]
    fn appcontainer_denies_internet_by_default() {
        // 默认 network_mode == Enforce。
        let cfg = policy_to_appcontainer_config(&SandboxPolicy::default());
        assert!(!cfg.internet_granted);
        assert!(cfg.capabilities.is_empty());
    }

    #[test]
    fn appcontainer_grants_internet_when_off() {
        let policy = SandboxPolicy {
            network_mode: NetworkMode::Off,
            ..Default::default()
        };
        let cfg = policy_to_appcontainer_config(&policy);
        assert!(cfg.internet_granted);
        assert!(cfg
            .capabilities
            .contains(&AppContainerCapability::InternetClient));
    }

    #[test]
    fn appcontainer_enforce_still_blocks_internet() {
        let policy = SandboxPolicy {
            network_mode: NetworkMode::Enforce,
            ..Default::default()
        };
        let cfg = policy_to_appcontainer_config(&policy);
        assert!(!cfg.internet_granted);
    }

    #[test]
    fn appcontainer_maps_filesystem_paths() {
        let policy = SandboxPolicy {
            filesystem: FilesystemPolicy {
                read_roots: vec![PathBuf::from("C:/ws")],
                write_roots: vec![PathBuf::from("C:/tmp")],
                deny: vec![PathBuf::from("C:/secrets")],
            },
            ..Default::default()
        };
        let cfg = policy_to_appcontainer_config(&policy);
        assert_eq!(cfg.read_paths, vec![PathBuf::from("C:/ws")]);
        assert_eq!(cfg.write_paths, vec![PathBuf::from("C:/tmp")]);
        assert_eq!(cfg.denied_paths, vec![PathBuf::from("C:/secrets")]);
    }

    #[test]
    fn probe_reports_unavailable_with_reason() {
        let outcome = probe_appcontainer_job();
        assert!(!outcome.available);
        assert!(!outcome.reason.is_empty());
    }
}
