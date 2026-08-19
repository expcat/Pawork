//! Windows AppContainer 配置/探测与 Job Object 执行后端。
//!
//! 配置纯函数 [`policy_to_appcontainer_config`] 跨平台编译并单测；
//! [`probe_appcontainer_job`] 在 Windows 经 kernel32 `IsProcessInJob` 真实探测
//! 当前进程是否已身处 Job（影响 Job 嵌套），非 Windows 返回不可用 stub。
//!
//! AppContainer 受限令牌 spawn 仍需要 `EXTENDED_STARTUPINFO_PRESENT`，因此探测结果
//! 明确标记不可用（frozen，`available: false`）；可执行路径使用 Job Object。

use crate::sandbox::{NetworkMode, ProbeOutcome, SandboxPolicy};
use std::sync::OnceLock;

/// AppContainer capability（最小权限集；默认不授予 Internet 以实现网络隔离）。
// frozen, awaiting AppContainer restricted-token spawn：生成器仅保留供诊断/审计与单测。
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
// frozen, awaiting AppContainer spawn：无 spawn 消费方，保留至后端接入。
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
// frozen, awaiting AppContainer spawn 后端未接入，生成器保留不删。
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
pub(crate) use job::{resume, Job};

#[cfg(windows)]
mod job {
    use std::collections::{HashMap, HashSet};
    use std::io;
    use std::mem::size_of;

    use tokio::process::Child;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, STILL_ACTIVE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
        JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
        JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_JOB_TIME, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, TerminateProcess,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    use crate::process::ProcessLimits;

    #[link(name = "ntdll")]
    extern "system" {
        fn NtResumeProcess(process_handle: HANDLE) -> i32;
    }

    /// 独占拥有的 Job Object；Drop 关闭句柄并触发 KILL_ON_JOB_CLOSE。
    #[derive(Debug)]
    pub(crate) struct Job(HANDLE);

    // HANDLE 代表唯一拥有的内核句柄，所有访问均通过线程安全的 Win32 API。
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        fn create(limits: ProcessLimits) -> io::Result<Self> {
            let handle = unsafe { CreateJobObjectW(None, None) }.map_err(io::Error::other)?;
            let job = Self(handle);

            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            let mut flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE.0;
            if let Some(max_processes) = limits.max_processes {
                info.BasicLimitInformation.ActiveProcessLimit = max_processes.max(1);
                flags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS.0;
            }
            if let Some(memory_bytes) = limits.memory_bytes {
                info.JobMemoryLimit = usize::try_from(memory_bytes).unwrap_or(usize::MAX);
                flags |= JOB_OBJECT_LIMIT_JOB_MEMORY.0;
            }
            if let Some(cpu_time) = limits.cpu_time {
                let intervals_100ns = cpu_time.as_nanos() / 100;
                info.BasicLimitInformation.PerJobUserTimeLimit =
                    i64::try_from(intervals_100ns).unwrap_or(i64::MAX);
                flags |= JOB_OBJECT_LIMIT_JOB_TIME.0;
            }
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT(flags);

            unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                        .expect("job information size fits u32"),
                )
            }
            .map_err(io::Error::other)?;

            Ok(job)
        }

        pub(crate) fn attach(child: &Child, limits: ProcessLimits) -> io::Result<Self> {
            let job = Self::create(limits)?;

            let process = HANDLE(child.raw_handle().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "child has no process handle")
            })?);
            unsafe { AssignProcessToJobObject(job.0, process) }.map_err(io::Error::other)?;
            Ok(job)
        }

        pub(crate) fn attach_pid(process_id: u32, limits: ProcessLimits) -> io::Result<Self> {
            let job = Self::create(limits)?;
            job.assign_pid(process_id)?;
            // portable-pty/ConPTY 不暴露 suspended creation。根进程绑定后，未来子进程会
            // 自动继承 Job；这里再收编绑定窗口内已经产生的后代，避免它们逃逸。
            job.adopt_existing_descendants(process_id)?;
            Ok(job)
        }

        fn assign_pid(&self, process_id: u32) -> io::Result<()> {
            let process = unsafe {
                OpenProcess(
                    PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                    false,
                    process_id,
                )
            }
            .map_err(io::Error::other)?;

            let assigned = unsafe { AssignProcessToJobObject(self.0, process) };
            if let Err(error) = assigned {
                let mut in_job = BOOL::default();
                let already_assigned = unsafe {
                    IsProcessInJob(process, Some(self.0), &mut in_job).is_ok() && in_job.as_bool()
                };
                if !already_assigned {
                    let mut exit_code = 0u32;
                    let still_running = unsafe { GetExitCodeProcess(process, &mut exit_code) }
                        .is_ok()
                        && exit_code == STILL_ACTIVE.0 as u32;
                    if !still_running {
                        let _ = unsafe { CloseHandle(process) };
                        return Ok(());
                    }
                    // 不允许绑定到同一 Job 的后代不能继续存活，否则调用方会误以为
                    // ProcessTreeGuard 覆盖完整进程树。
                    let _ = unsafe { TerminateProcess(process, 1) };
                    let _ = unsafe { CloseHandle(process) };
                    return Err(io::Error::other(format!(
                        "failed to assign process {process_id} to Job Object: {error}"
                    )));
                }
            }
            let _ = unsafe { CloseHandle(process) };
            Ok(())
        }

        fn pid_in_job(&self, process_id: u32) -> io::Result<Option<bool>> {
            let process = match unsafe {
                OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id)
            } {
                Ok(process) => process,
                // 进程可能在 snapshot 后退出；下一轮重新枚举即可区分。
                Err(_) => return Ok(None),
            };
            let mut in_job = BOOL::default();
            let result = unsafe { IsProcessInJob(process, Some(self.0), &mut in_job) };
            let _ = unsafe { CloseHandle(process) };
            result.map_err(io::Error::other)?;
            Ok(Some(in_job.as_bool()))
        }

        fn adopt_existing_descendants(&self, root_process_id: u32) -> io::Result<()> {
            const MAX_ADOPTION_ROUNDS: usize = 16;
            let mut last_unresolved = Vec::new();

            for _ in 0..MAX_ADOPTION_ROUNDS {
                let descendants = descendant_process_ids(root_process_id)?;
                let mut unresolved = Vec::new();
                for process_id in descendants {
                    match self.pid_in_job(process_id)? {
                        Some(true) => {}
                        Some(false) => self.assign_pid(process_id)?,
                        None => unresolved.push(process_id),
                    }
                }

                // 重新 snapshot：已绑定进程今后创建的后代会自动继承；只有在绑定前
                // 产生的后代还可能是新成员。连续一轮无未绑定成员即可收敛。
                let verification = descendant_process_ids(root_process_id)?;
                let mut all_assigned = true;
                for process_id in verification {
                    if self.pid_in_job(process_id)? != Some(true) {
                        all_assigned = false;
                        unresolved.push(process_id);
                    }
                }
                unresolved.sort_unstable();
                unresolved.dedup();
                if all_assigned {
                    return Ok(());
                }
                last_unresolved = unresolved;
                std::thread::yield_now();
            }

            Err(io::Error::other(format!(
                "failed to adopt pre-existing descendants into Job Object: {last_unresolved:?}"
            )))
        }

        pub(crate) fn terminate(&self) -> io::Result<()> {
            unsafe { TerminateJobObject(self.0, 1) }.map_err(io::Error::other)
        }
    }

    pub(crate) fn resume(child: &Child) -> io::Result<()> {
        let process = HANDLE(child.raw_handle().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "child has no process handle")
        })?);
        let status = unsafe { NtResumeProcess(process) };
        if status >= 0 {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "NtResumeProcess failed with NTSTATUS 0x{:08x}",
                status as u32
            )))
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    fn descendant_process_ids(root_process_id: u32) -> io::Result<Vec<u32>> {
        let root_creation_time = process_creation_time(root_process_id)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("root process {root_process_id} exited before Job adoption"),
            )
        })?;
        let snapshot =
            unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.map_err(io::Error::other)?;
        let mut entry = PROCESSENTRY32W {
            dwSize: u32::try_from(size_of::<PROCESSENTRY32W>())
                .expect("process entry size fits u32"),
            ..Default::default()
        };
        let mut parents = HashMap::new();
        if unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok() {
            loop {
                parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
                if unsafe { Process32NextW(snapshot, &mut entry) }.is_err() {
                    break;
                }
            }
        }
        let _ = unsafe { CloseHandle(snapshot) };

        let mut descendants = Vec::new();
        for process_id in parents.keys().copied() {
            let mut current = process_id;
            let mut visited = HashSet::new();
            let mut is_descendant = false;
            while let Some(parent) = parents.get(&current).copied() {
                if parent == root_process_id {
                    is_descendant = true;
                    break;
                }
                if parent == 0 || !visited.insert(parent) {
                    break;
                }
                current = parent;
            }
            if is_descendant
                && matches!(
                    process_creation_time(process_id)?,
                    Some(created) if created >= root_creation_time
                )
            {
                descendants.push(process_id);
            }
        }
        Ok(descendants)
    }

    fn process_creation_time(process_id: u32) -> io::Result<Option<u64>> {
        let process =
            match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) } {
                Ok(process) => process,
                Err(_) => return Ok(None),
            };
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let result =
            unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) };
        let _ = unsafe { CloseHandle(process) };
        result.map_err(io::Error::other)?;
        Ok(Some(
            (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime),
        ))
    }
}

#[cfg(windows)]
mod job_backend {
    use crate::cancel::CancellationToken;
    use crate::process::ProcessRuntime;
    use crate::sandbox::{
        apply_soft_restrictions, NetworkMode, SandboxBackend, SandboxError,
        SandboxInteractiveProcess, SandboxPolicy, SandboxProcess, SandboxProcessSpec,
    };
    use async_trait::async_trait;

    /// Windows 可执行兜底：ProcessRuntime 会为每个子进程建立 Job Object，施加
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
            Ok(SandboxProcess::new(events, handle))
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
                    backend = "windows_job",
                    "AppContainer unavailable; Job Object enforces process/resource limits but not filesystem/network isolation"
                );
            }
            let (events, input, handle) = self
                .runtime
                .spawn_interactive(spec.command, cancel)
                .await
                .map_err(SandboxError::Process)?;
            Ok(SandboxInteractiveProcess::new(events, input, handle))
        }
    }
}

#[cfg(windows)]
pub use job_backend::WindowsJobBackend;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::FilesystemPolicy;
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
