//! Linux 进程树、Landlock restrict，以及 bwrap / Landlock 沙箱后端。
//!
//! 纯函数 [`generate_bwrap_argv`] 与 [`probe_landlock_support`] 在所有平台编译并单测；
//! spawn 后端与内核能力探测仅在 Linux 编译。

use std::path::PathBuf;
use std::sync::OnceLock;

use crate::sandbox::{NetworkMode, SandboxPolicy};

/// 系统只读路径单一来源：bwrap ro-bind 与 Landlock read_paths 共用，
/// 各消费方按需筛选（bwrap 跳过 /proc 与 /dev，由 --proc/--dev 挂载）。
// 跨平台编译供单测复用；非 Linux 宿主上无生产消费方，dead_code 属预期。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const SYSTEM_READ_PATHS: &[&str] = &[
    "/usr",
    "/lib",
    "/lib64",
    "/bin",
    "/sbin",
    "/nix",
    "/proc",
    "/etc/ld.so.cache",
    "/etc/ld.so.preload",
    "/etc/ssl",
    "/etc/ca-certificates",
    "/etc/resolv.conf",
    "/etc/hosts",
    "/etc/nsswitch.conf",
    "/etc/passwd",
    "/etc/group",
    "/dev/urandom",
    "/dev/random",
    "/dev/null",
    "/dev/zero",
];

/// 探测当前 Linux 内核是否能创建至少一个 Landlock 文件系统 ruleset。
#[cfg(target_os = "linux")]
pub(crate) fn linux_landlock_supported() -> Result<(), String> {
    std::thread::Builder::new()
        .name("pawork-landlock-probe".into())
        .spawn(|| {
            use std::os::fd::AsRawFd;

            let ruleset = prepare_linux_landlock(&crate::process::LinuxLandlockPolicy::default())?;
            // Landlock 默认只限制调用线程；探测线程随即退出，不影响调用方其他线程。
            restrict_linux_landlock(ruleset.as_raw_fd())
        })
        .map_err(|error| error.to_string())?
        .join()
        .map_err(|_| "Landlock probe thread panicked".to_string())?
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "linux")]
pub(crate) fn restrict_linux_landlock(ruleset_fd: std::os::fd::RawFd) -> std::io::Result<()> {
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset_fd, 0) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn prepare_linux_landlock(
    policy: &crate::process::LinuxLandlockPolicy,
) -> std::io::Result<std::os::fd::OwnedFd> {
    use std::collections::BTreeSet;
    use std::os::fd::OwnedFd;

    use landlock::{
        Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr,
        RulesetCreatedAttr, ABI,
    };

    fn canonical_paths(
        paths: &[std::path::PathBuf],
    ) -> std::io::Result<BTreeSet<std::path::PathBuf>> {
        paths
            .iter()
            .map(std::fs::canonicalize)
            .collect::<Result<_, _>>()
    }

    fn add_paths(
        mut ruleset: landlock::RulesetCreated,
        paths: BTreeSet<std::path::PathBuf>,
        directory_access: landlock::BitFlags<AccessFs>,
        abi: ABI,
    ) -> std::io::Result<landlock::RulesetCreated> {
        let file_access = directory_access & AccessFs::from_file(abi);
        for path in paths {
            let metadata = std::fs::metadata(&path)?;
            let access = if metadata.is_dir() {
                directory_access
            } else {
                file_access
            };
            let parent = PathFd::new(&path).map_err(|error| {
                std::io::Error::other(format!("open Landlock path {}: {error}", path.display()))
            })?;
            ruleset = ruleset
                .add_rule(PathBeneath::new(parent, access))
                .map_err(|error| {
                    std::io::Error::other(format!(
                        "add Landlock rule for {}: {error}",
                        path.display()
                    ))
                })?;
        }
        Ok(ruleset)
    }

    let abi = ABI::V9;
    // BestEffort 会在旧内核上剔除较新的访问位；若内核完全不支持 Landlock，
    // create() 最终不会产生 FD，下面仍会硬失败而不是静默降级。
    let ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(abi))
        .map_err(|error| std::io::Error::other(error.to_string()))?
        .create()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let ruleset = add_paths(
        ruleset,
        canonical_paths(&policy.read_paths)?,
        AccessFs::from_read(abi),
        abi,
    )?;
    let ruleset = add_paths(
        ruleset,
        canonical_paths(&policy.write_paths)?,
        AccessFs::from_all(abi),
        abi,
    )?;
    Option::<OwnedFd>::from(ruleset).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Landlock is not enforced by the running kernel",
        )
    })
}

#[cfg(target_os = "linux")]
pub(crate) mod linux_process_tree {
    use std::collections::{HashMap, HashSet};
    use std::io;

    const MAX_FREEZE_ROUNDS: usize = 16;

    #[derive(Clone, Copy, Debug)]
    struct ProcessRecord {
        pid: i32,
        ppid: i32,
        pgrp: i32,
        start_time: u64,
    }

    fn parse_stat(text: &str) -> io::Result<ProcessRecord> {
        let open = text
            .find('(')
            .ok_or_else(|| io::Error::other("missing comm"))?;
        let close = text
            .rfind(')')
            .ok_or_else(|| io::Error::other("missing comm terminator"))?;
        let pid = text[..open]
            .trim()
            .parse::<i32>()
            .map_err(io::Error::other)?;
        let fields = text[close + 1..].split_whitespace().collect::<Vec<_>>();
        if fields.len() <= 19 {
            return Err(io::Error::other("short /proc stat record"));
        }
        Ok(ProcessRecord {
            pid,
            ppid: fields[1].parse::<i32>().map_err(io::Error::other)?,
            pgrp: fields[2].parse::<i32>().map_err(io::Error::other)?,
            start_time: fields[19].parse::<u64>().map_err(io::Error::other)?,
        })
    }

    fn read_process(pid: i32) -> io::Result<Option<ProcessRecord>> {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => parse_stat(&stat).map(Some),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn snapshot() -> io::Result<HashMap<i32, ProcessRecord>> {
        let mut processes = HashMap::new();
        for entry in std::fs::read_dir("/proc")? {
            let Ok(entry) = entry else { continue };
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<i32>().ok())
            else {
                continue;
            };
            if let Some(process) = read_process(pid)? {
                processes.insert(pid, process);
            }
        }
        Ok(processes)
    }

    pub(crate) fn start_time(pid: i32) -> io::Result<u64> {
        read_process(pid)?
            .map(|process| process.start_time)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("process {pid} exited before tree guard attachment"),
                )
            })
    }

    fn descendants(
        processes: &HashMap<i32, ProcessRecord>,
        root_pid: i32,
        root_start_time: u64,
    ) -> Vec<(ProcessRecord, usize)> {
        let mut descendants = Vec::new();
        for process in processes.values().copied() {
            if process.pid == root_pid || process.start_time < root_start_time {
                continue;
            }
            let mut current = process;
            let mut depth = 0usize;
            let mut visited = HashSet::new();
            while current.ppid > 1 && visited.insert(current.pid) {
                depth += 1;
                if current.ppid == root_pid {
                    descendants.push((process, depth));
                    break;
                }
                let Some(parent) = processes.get(&current.ppid).copied() else {
                    break;
                };
                current = parent;
            }
        }
        descendants
    }

    fn signal_raw(pid: i32, signal: i32) -> io::Result<()> {
        if unsafe { libc::kill(pid, signal) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }

    fn signal_group(pgid: i32, signal: i32) -> io::Result<()> {
        if unsafe { libc::killpg(pgid, signal) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }

    fn signal_record(process: ProcessRecord, signal: i32) -> io::Result<()> {
        match read_process(process.pid)? {
            Some(current) if current.start_time == process.start_time => {
                signal_raw(process.pid, signal)
            }
            _ => Ok(()),
        }
    }

    fn remember_error(slot: &mut Option<io::Error>, result: io::Result<()>) {
        if slot.is_none() {
            if let Err(error) = result {
                *slot = Some(error);
            }
        }
    }

    pub(crate) fn terminate(root_pid: i32, pgid: i32, root_start_time: u64) -> io::Result<()> {
        let initial = snapshot()?;
        match initial.get(&root_pid) {
            Some(root) if root.start_time != root_start_time => {
                // PID/PGID 已复用，绝不能杀死新的无关进程树。
                return Ok(());
            }
            None if !initial
                .values()
                .any(|process| process.pgrp == pgid && process.start_time >= root_start_time) =>
            {
                return Ok(());
            }
            _ => {}
        }

        let mut first_error = None;
        // 先冻结原 process group，阻止仍在组内的进程继续派生；再逐轮冻结已
        // `setsid` 的后代，直至 snapshot 收敛。
        remember_error(&mut first_error, signal_group(pgid, libc::SIGSTOP));
        let mut frozen = HashMap::<i32, (ProcessRecord, usize)>::new();
        for _ in 0..MAX_FREEZE_ROUNDS {
            let processes = match snapshot() {
                Ok(processes) => processes,
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                    break;
                }
            };
            if matches!(
                processes.get(&root_pid),
                Some(root) if root.start_time != root_start_time
            ) {
                break;
            }
            let mut added = 0usize;
            for (process, depth) in descendants(&processes, root_pid, root_start_time) {
                let is_new = frozen
                    .get(&process.pid)
                    .is_none_or(|(known, _)| known.start_time != process.start_time);
                if is_new {
                    remember_error(&mut first_error, signal_record(process, libc::SIGSTOP));
                    frozen.insert(process.pid, (process, depth));
                    added += 1;
                }
            }
            if added == 0 {
                break;
            }
            std::thread::yield_now();
        }

        let mut descendants = frozen.into_values().collect::<Vec<_>>();
        descendants.sort_unstable_by_key(|(_, depth)| std::cmp::Reverse(*depth));
        for (process, _) in descendants {
            remember_error(&mut first_error, signal_record(process, libc::SIGKILL));
        }
        remember_error(&mut first_error, signal_group(pgid, libc::SIGKILL));
        first_error.map_or(Ok(()), Err)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_proc_stat_with_spaces_and_parentheses_in_comm() {
            let mut fields = vec!["0"; 20];
            fields[0] = "S";
            fields[1] = "7";
            fields[2] = "42";
            fields[19] = "1234";
            let stat = format!("42 (odd ) command) {}", fields.join(" "));
            let record = parse_stat(&stat).expect("parse");
            assert_eq!(record.pid, 42);
            assert_eq!(record.ppid, 7);
            assert_eq!(record.pgrp, 42);
            assert_eq!(record.start_time, 1234);
        }
    }
}

/// landlock 能力探测结果。
// 跨平台编译供单测复用；非 Linux 宿主上无生产消费方，dead_code 属预期。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Debug)]
pub struct LandlockSupport {
    /// 内核是否启用了 landlock LSM（能力层面）。
    pub supported: bool,
    pub reason: String,
}

/// 通过真实 ruleset 创建探测 Landlock，而非依赖可能不可读的 `/sys` 状态文件。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn probe_landlock_support() -> LandlockSupport {
    static SUPPORT: OnceLock<LandlockSupport> = OnceLock::new();
    SUPPORT
        .get_or_init(|| {
            #[cfg(target_os = "linux")]
            {
                match linux_landlock_supported() {
                    Ok(()) => LandlockSupport {
                        supported: true,
                        reason: "Landlock ruleset creation succeeded".into(),
                    },
                    Err(error) => LandlockSupport {
                        supported: false,
                        reason: format!("Landlock ruleset creation failed: {error}"),
                    },
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                LandlockSupport {
                    supported: false,
                    reason: "Landlock is only available on Linux".into(),
                }
            }
        })
        .clone()
}

/// 把 [`SandboxPolicy`] 编译为 bwrap argv（不含末尾 `--` 与内部命令）。
///
/// 强项：文件系统 bind（read/write）、网络 `--unshare-net`（Enforce）、
/// 进程 `--unshare-pid`、生命周期 `--die-with-parent`。系统只读目录仅绑定实际存在的。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn generate_bwrap_argv(policy: &SandboxPolicy, workspace_roots: &[PathBuf]) -> Vec<String> {
    let mut argv: Vec<String> = Vec::new();
    // 与 Landlock 共用 SYSTEM_READ_PATHS 单一来源；/proc 与 /dev/* 由
    // --proc/--dev 挂载，不重复 ro-bind；仅绑定实际存在的路径。
    for host in SYSTEM_READ_PATHS {
        if host.starts_with("/proc") || host.starts_with("/dev") {
            continue;
        }
        if std::path::Path::new(host).exists() {
            argv.push("--ro-bind".into());
            argv.push((*host).into());
            argv.push((*host).into());
        }
    }
    argv.push("--dev".into());
    argv.push("/dev".into());
    argv.push("--proc".into());
    argv.push("/proc".into());
    // 先只读绑定全部可见根，再用更具体的写根覆盖；workspace 本身默认只读，
    // 只有 policy.write_roots 明确授权的路径才使用 --bind。
    for r in &policy.filesystem.read_roots {
        let p = r.to_string_lossy().to_string();
        argv.push("--ro-bind".into());
        argv.push(p.clone());
        argv.push(p);
    }
    for root in workspace_roots {
        let p = root.to_string_lossy().to_string();
        argv.push("--ro-bind".into());
        argv.push(p.clone());
        argv.push(p);
    }
    for w in &policy.filesystem.write_roots {
        let p = w.to_string_lossy().to_string();
        argv.push("--bind".into());
        argv.push(p.clone());
        argv.push(p);
    }
    // deny 位于已绑定根内时用空 tmpfs 覆盖，避免上层 bind 暴露 Secret 子目录。
    for denied in &policy.filesystem.deny {
        let visible = policy
            .filesystem
            .read_roots
            .iter()
            .chain(&policy.filesystem.write_roots)
            .chain(workspace_roots)
            .any(|root| crate::path::path_within_root(denied, root));
        if visible {
            argv.push("--tmpfs".into());
            argv.push(denied.to_string_lossy().to_string());
        }
    }
    if policy.network_mode == NetworkMode::Enforce {
        argv.push("--unshare-net".into());
    }
    argv.push("--unshare-pid".into());
    argv.push("--unshare-ipc".into());
    argv.push("--unshare-uts".into());
    argv.push("--unshare-cgroup-try".into());
    argv.push("--die-with-parent".into());
    argv.push("--new-session".into());
    argv
}

#[cfg(target_os = "linux")]
mod bwrap {
    use super::generate_bwrap_argv;
    use crate::cancel::CancellationToken;
    use crate::process::ProcessRuntime;
    use crate::sandbox::{
        apply_soft_restrictions, SandboxBackend, SandboxError, SandboxInteractiveProcess,
        SandboxPolicy, SandboxProcess, SandboxProcessSpec,
    };
    use async_trait::async_trait;
    use std::sync::OnceLock;

    fn run_probe() -> (bool, String) {
        match std::process::Command::new("bwrap")
            .args(["--die-with-parent", "--ro-bind", "/", "/", "--", "true"])
            .output()
        {
            Ok(out) if out.status.success() => (true, String::new()),
            Ok(out) => (
                false,
                format!(
                    "bwrap namespace smoke probe exited non-zero: {:?}",
                    out.status
                ),
            ),
            Err(e) => (false, format!("bwrap not found or not executable: {e}")),
        }
    }

    pub(super) fn probe() -> &'static (bool, String) {
        static PROBE: OnceLock<(bool, String)> = OnceLock::new();
        PROBE.get_or_init(run_probe)
    }

    pub fn bwrap_probe_reason() -> String {
        probe().1.clone()
    }

    /// Linux Bubblewrap（bwrap）硬隔离后端。
    #[derive(Clone, Debug)]
    pub struct BwrapBackend {
        runtime: ProcessRuntime,
        available: bool,
    }

    impl BwrapBackend {
        pub fn with_runtime(runtime: ProcessRuntime) -> Self {
            let available = probe().0;
            Self { runtime, available }
        }
    }

    #[async_trait]
    impl SandboxBackend for BwrapBackend {
        fn id(&self) -> &'static str {
            "bwrap"
        }

        fn available(&self) -> bool {
            self.available
        }

        async fn spawn(
            &self,
            mut spec: SandboxProcessSpec,
            policy: SandboxPolicy,
            cancel: CancellationToken,
        ) -> Result<SandboxProcess, SandboxError> {
            if !self.available {
                return Err(SandboxError::BackendUnavailable("bwrap"));
            }
            apply_soft_restrictions(&mut spec, &policy)?;
            let bwrap_args = generate_bwrap_argv(&policy, &spec.workspace_roots);
            let inner_program = spec.command.program.clone();
            let inner_args = std::mem::take(&mut spec.command.args);
            let mut argv = bwrap_args;
            argv.push("--".into());
            argv.push(inner_program);
            argv.extend(inner_args);
            spec.command.program = "bwrap".into();
            spec.command.args = argv;
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
            if !self.available {
                return Err(SandboxError::BackendUnavailable("bwrap"));
            }
            apply_soft_restrictions(&mut spec, &policy)?;
            let bwrap_args = generate_bwrap_argv(&policy, &spec.workspace_roots);
            let inner_program = spec.command.program.clone();
            let inner_args = std::mem::take(&mut spec.command.args);
            let mut argv = bwrap_args;
            argv.push("--".into());
            argv.push(inner_program);
            argv.extend(inner_args);
            spec.command.program = "bwrap".into();
            spec.command.args = argv;
            let (events, input, handle) = self
                .runtime
                .spawn_interactive(spec.command, cancel)
                .await
                .map_err(SandboxError::Process)?;
            Ok(SandboxInteractiveProcess::new(events, input, handle))
        }
    }
}

#[cfg(target_os = "linux")]
pub use bwrap::{bwrap_probe_reason, BwrapBackend};

#[cfg(target_os = "linux")]
mod landlock_backend {
    use std::path::{Path, PathBuf};

    use async_trait::async_trait;

    use super::{probe_landlock_support, SYSTEM_READ_PATHS};
    use crate::cancel::CancellationToken;
    use crate::path::{canonicalize_platform, path_within_root};
    use crate::process::{LinuxLandlockPolicy, ProcessRuntime};
    use crate::sandbox::{
        apply_soft_restrictions, NetworkMode, SandboxBackend, SandboxError,
        SandboxInteractiveProcess, SandboxPolicy, SandboxProcess, SandboxProcessSpec,
    };

    const SYSTEM_WRITE_PATHS: &[&str] = &["/dev/null", "/dev/zero"];

    fn normalized_or_original(path: &Path) -> PathBuf {
        canonicalize_platform(path).unwrap_or_else(|_| path.to_path_buf())
    }

    fn overlaps(left: &Path, right: &Path) -> bool {
        path_within_root(left, right) || path_within_root(right, left)
    }

    fn denied(path: &Path, deny: &[PathBuf]) -> bool {
        let path = normalized_or_original(path);
        deny.iter()
            .map(|item| normalized_or_original(item))
            .any(|item| path_within_root(&path, &item))
    }

    fn resolved_executable(spec: &SandboxProcessSpec) -> Option<PathBuf> {
        use std::os::unix::fs::PermissionsExt;

        fn executable(path: PathBuf) -> Option<PathBuf> {
            let metadata = std::fs::metadata(&path).ok()?;
            if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
                Some(normalized_or_original(&path))
            } else {
                None
            }
        }

        let program = Path::new(&spec.command.program);
        if program.is_absolute() {
            return executable(program.to_path_buf());
        }
        if program.components().count() > 1 {
            return spec
                .command
                .cwd
                .as_ref()
                .map(|cwd| cwd.join(program))
                .and_then(executable);
        }
        spec.command
            .env
            .iter()
            .rev()
            .find(|(name, _)| name == "PATH")
            .into_iter()
            .flat_map(|(_, value)| std::env::split_paths(value))
            .find_map(|directory| executable(directory.join(program)))
    }

    fn compile_policy(
        spec: &SandboxProcessSpec,
        policy: &SandboxPolicy,
    ) -> Result<LinuxLandlockPolicy, SandboxError> {
        let explicit_roots = policy
            .filesystem
            .read_roots
            .iter()
            .chain(&policy.filesystem.write_roots)
            .chain(&spec.workspace_roots);
        for allowed in explicit_roots {
            let allowed = normalized_or_original(allowed);
            for denied_path in &policy.filesystem.deny {
                let denied_path = normalized_or_original(denied_path);
                if overlaps(&allowed, &denied_path) {
                    return Err(SandboxError::Denied(format!(
                        "Landlock cannot subtract denied path {} from allowed root {}",
                        denied_path.display(),
                        allowed.display()
                    )));
                }
            }
        }

        let mut read_paths = policy.filesystem.read_roots.clone();
        read_paths.extend(spec.workspace_roots.iter().cloned());
        let mut write_paths = policy.filesystem.write_roots.clone();

        if let Some(cwd) = &spec.command.cwd {
            read_paths.push(cwd.clone());
        }
        if let Some(program) = resolved_executable(spec) {
            if !denied(&program, &policy.filesystem.deny) {
                // 仅授予已解析的 executable 文件，不把宿主 PATH 目录整树暴露给命令。
                read_paths.push(program);
            }
        }
        read_paths.extend(
            SYSTEM_READ_PATHS
                .iter()
                .map(PathBuf::from)
                .filter(|path| path.exists() && !denied(path, &policy.filesystem.deny)),
        );
        write_paths.extend(
            SYSTEM_WRITE_PATHS
                .iter()
                .map(PathBuf::from)
                .filter(|path| path.exists() && !denied(path, &policy.filesystem.deny)),
        );

        Ok(LinuxLandlockPolicy {
            read_paths,
            write_paths,
        })
    }

    /// Landlock 文件系统硬隔离后端；网络与 PID namespace 不在其保证范围内。
    #[derive(Clone, Debug)]
    pub struct LandlockBackend {
        runtime: ProcessRuntime,
        available: bool,
    }

    impl LandlockBackend {
        pub fn with_runtime(runtime: ProcessRuntime) -> Self {
            Self {
                runtime,
                available: probe_landlock_support().supported,
            }
        }
    }

    #[async_trait]
    impl SandboxBackend for LandlockBackend {
        fn id(&self) -> &'static str {
            "landlock"
        }

        fn available(&self) -> bool {
            self.available
        }

        async fn spawn(
            &self,
            mut spec: SandboxProcessSpec,
            policy: SandboxPolicy,
            cancel: CancellationToken,
        ) -> Result<SandboxProcess, SandboxError> {
            if !self.available {
                return Err(SandboxError::BackendUnavailable("landlock"));
            }
            apply_soft_restrictions(&mut spec, &policy)?;
            if policy.network_mode == NetworkMode::Enforce {
                tracing::warn!(
                    target: "pawork.sandbox",
                    backend = "landlock",
                    "Landlock enforces filesystem access only; network policy is not hard-enforced"
                );
            }
            spec.command.landlock = Some(compile_policy(&spec, &policy)?);
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
            if !self.available {
                return Err(SandboxError::BackendUnavailable("landlock"));
            }
            apply_soft_restrictions(&mut spec, &policy)?;
            if policy.network_mode == NetworkMode::Enforce {
                tracing::warn!(
                    target: "pawork.sandbox",
                    backend = "landlock",
                    "Landlock enforces filesystem access only; network policy is not hard-enforced"
                );
            }
            spec.command.landlock = Some(compile_policy(&spec, &policy)?);
            let (events, input, handle) = self
                .runtime
                .spawn_interactive(spec.command, cancel)
                .await
                .map_err(SandboxError::Process)?;
            Ok(SandboxInteractiveProcess::new(events, input, handle))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::process::{CommandSpec, ProcessEvent};
        use crate::sandbox::FilesystemPolicy;

        #[test]
        fn nested_deny_is_rejected_instead_of_silently_exposed() {
            let root = std::env::temp_dir();
            let policy = SandboxPolicy {
                filesystem: FilesystemPolicy {
                    read_roots: vec![root.clone()],
                    deny: vec![root.join("secret")],
                    ..Default::default()
                },
                ..Default::default()
            };
            let spec = SandboxProcessSpec {
                command: CommandSpec::new("true"),
                workspace_roots: vec![root],
            };
            assert!(matches!(
                compile_policy(&spec, &policy),
                Err(SandboxError::Denied(_))
            ));
        }

        #[test]
        fn custom_path_grants_only_the_resolved_executable() {
            use std::os::unix::fs::PermissionsExt;

            let temp = tempfile::tempdir().expect("tempdir");
            let workspace = temp.path().join("workspace");
            let tools = temp.path().join("tools");
            std::fs::create_dir_all(&workspace).expect("workspace");
            std::fs::create_dir_all(&tools).expect("tools");
            let executable = tools.join("pawork-test-tool");
            std::fs::write(&executable, b"#!/bin/sh\nexit 0\n").expect("write executable");
            let mut permissions = std::fs::metadata(&executable)
                .expect("metadata")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&executable, permissions).expect("chmod");

            let mut command = CommandSpec::new("pawork-test-tool");
            command.cwd = Some(workspace.clone());
            command.env.push((
                "PATH".into(),
                std::env::join_paths([tools.clone()])
                    .expect("join PATH")
                    .to_string_lossy()
                    .into_owned(),
            ));
            let spec = SandboxProcessSpec {
                command,
                workspace_roots: vec![workspace.clone()],
            };
            let policy = SandboxPolicy {
                filesystem: FilesystemPolicy {
                    read_roots: vec![workspace],
                    ..Default::default()
                },
                ..Default::default()
            };

            let compiled = compile_policy(&spec, &policy).expect("compile policy");
            let executable = normalized_or_original(&executable);
            let tools = normalized_or_original(&tools);
            assert!(compiled.read_paths.contains(&executable));
            assert!(!compiled.read_paths.contains(&tools));
        }

        #[tokio::test]
        async fn landlock_allows_workspace_and_denies_sibling_file() {
            let backend = LandlockBackend::with_runtime(ProcessRuntime::new());
            if !backend.available() {
                return;
            }

            let temp = tempfile::tempdir().expect("tempdir");
            let workspace = temp.path().join("workspace");
            std::fs::create_dir(&workspace).expect("workspace");
            let allowed = workspace.join("allowed.txt");
            let outside = temp.path().join("outside.txt");
            std::fs::write(&allowed, b"allowed-canary").expect("allowed file");
            std::fs::write(&outside, b"outside-secret-canary").expect("outside file");

            let cat = if Path::new("/bin/cat").exists() {
                "/bin/cat"
            } else {
                "/usr/bin/cat"
            };
            let policy = SandboxPolicy {
                filesystem: FilesystemPolicy {
                    read_roots: vec![workspace.clone()],
                    write_roots: vec![workspace.clone()],
                    deny: Vec::new(),
                },
                network_mode: NetworkMode::Hint,
                allow_spawn: true,
                ..Default::default()
            };

            async fn cat_file(
                backend: &LandlockBackend,
                policy: SandboxPolicy,
                workspace: &Path,
                cat: &str,
                path: &Path,
            ) -> (Option<i32>, Vec<u8>) {
                let mut command = CommandSpec::new(cat).arg(path.to_string_lossy().into_owned());
                command.cwd = Some(workspace.to_path_buf());
                let mut process = backend
                    .spawn(
                        SandboxProcessSpec {
                            command,
                            workspace_roots: vec![workspace.to_path_buf()],
                        },
                        policy,
                        CancellationToken::new(),
                    )
                    .await
                    .expect("Landlock spawn");
                let mut exit = None;
                let mut output = Vec::new();
                while let Some(event) = process.events.recv().await {
                    match event {
                        ProcessEvent::Stdout(chunk) | ProcessEvent::Stderr(chunk) => {
                            output.extend(chunk)
                        }
                        ProcessEvent::Exit { code, .. } => {
                            exit = code;
                            break;
                        }
                    }
                }
                (exit, output)
            }

            let (allowed_exit, allowed_output) =
                cat_file(&backend, policy.clone(), &workspace, cat, &allowed).await;
            assert_eq!(allowed_exit, Some(0));
            assert!(allowed_output
                .windows(b"allowed-canary".len())
                .any(|window| window == b"allowed-canary"));

            let (outside_exit, outside_output) =
                cat_file(&backend, policy, &workspace, cat, &outside).await;
            assert_ne!(outside_exit, Some(0));
            assert!(!outside_output
                .windows(b"outside-secret-canary".len())
                .any(|window| window == b"outside-secret-canary"));
        }
    }
}

#[cfg(target_os = "linux")]
pub use landlock_backend::LandlockBackend;

#[cfg(not(target_os = "linux"))]
// 非 Linux 宿主的跨平台桩，仅单测消费。
#[allow(dead_code)]
pub fn bwrap_probe_reason() -> String {
    "bwrap only available on Linux".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::FilesystemPolicy;

    #[test]
    fn bwrap_argv_unshares_net_when_enforce() {
        let policy = SandboxPolicy {
            network_mode: NetworkMode::Enforce,
            ..Default::default()
        };
        let argv = generate_bwrap_argv(&policy, &[]);
        assert!(argv.contains(&"--unshare-net".to_string()));
    }

    #[test]
    fn bwrap_argv_keeps_net_when_not_enforce() {
        let policy = SandboxPolicy {
            network_mode: NetworkMode::Hint,
            ..Default::default()
        };
        let argv = generate_bwrap_argv(&policy, &[]);
        assert!(!argv.contains(&"--unshare-net".to_string()));
    }

    #[test]
    fn bwrap_argv_includes_lifecycle_and_pid_flags() {
        let argv = generate_bwrap_argv(&SandboxPolicy::default(), &[]);
        assert!(argv.contains(&"--unshare-pid".to_string()));
        assert!(argv.contains(&"--die-with-parent".to_string()));
        assert!(argv.contains(&"--new-session".to_string()));
        assert!(argv.contains(&"--dev".to_string()));
        assert!(argv.contains(&"--proc".to_string()));
    }

    #[test]
    fn bwrap_argv_binds_write_roots_rw() {
        let policy = SandboxPolicy {
            filesystem: FilesystemPolicy {
                write_roots: vec![PathBuf::from("/tmp/ws")],
                ..Default::default()
            },
            ..Default::default()
        };
        let argv = generate_bwrap_argv(&policy, &[]);
        let triple = [
            "--bind".to_string(),
            "/tmp/ws".to_string(),
            "/tmp/ws".to_string(),
        ];
        let found = argv.windows(3).any(|w| w == triple);
        assert!(found, "expected --bind /tmp/ws /tmp/ws in argv: {argv:?}");
    }

    #[test]
    fn bwrap_argv_binds_read_roots_ro() {
        let policy = SandboxPolicy {
            filesystem: FilesystemPolicy {
                read_roots: vec![PathBuf::from("/opt/ro")],
                ..Default::default()
            },
            ..Default::default()
        };
        let argv = generate_bwrap_argv(&policy, &[]);
        let triple = [
            "--ro-bind".to_string(),
            "/opt/ro".to_string(),
            "/opt/ro".to_string(),
        ];
        let found = argv.windows(3).any(|w| w == triple);
        assert!(
            found,
            "expected --ro-bind /opt/ro /opt/ro in argv: {argv:?}"
        );
    }

    #[test]
    fn landlock_probe_returns_reason() {
        let ll = probe_landlock_support();
        assert!(!ll.reason.is_empty());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn bwrap_allows_workspace_and_hides_unmounted_sibling() {
        use crate::cancel::CancellationToken;
        use crate::process::{CommandSpec, ProcessEvent, ProcessRuntime};
        use crate::sandbox::{SandboxBackend, SandboxProcessSpec};

        let backend = BwrapBackend::with_runtime(ProcessRuntime::new());
        if !backend.available() {
            return;
        }

        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir(&workspace).expect("workspace");
        let allowed = workspace.join("allowed.txt");
        let outside = temp.path().join("outside.txt");
        std::fs::write(&allowed, b"bwrap-allowed-canary").expect("allowed file");
        std::fs::write(&outside, b"bwrap-outside-secret").expect("outside file");

        let policy = SandboxPolicy {
            filesystem: FilesystemPolicy {
                read_roots: vec![workspace.clone()],
                write_roots: vec![workspace.clone()],
                deny: Vec::new(),
            },
            network_mode: NetworkMode::Hint,
            allow_spawn: true,
            ..Default::default()
        };
        let cat = if std::path::Path::new("/bin/cat").exists() {
            "/bin/cat"
        } else {
            "/usr/bin/cat"
        };

        async fn run_cat(
            backend: &BwrapBackend,
            policy: SandboxPolicy,
            workspace: &std::path::Path,
            cat: &str,
            path: &std::path::Path,
        ) -> (Option<i32>, Vec<u8>) {
            let mut command = CommandSpec::new(cat).arg(path.to_string_lossy().into_owned());
            command.cwd = Some(workspace.to_path_buf());
            let mut process = backend
                .spawn(
                    SandboxProcessSpec {
                        command,
                        workspace_roots: vec![workspace.to_path_buf()],
                    },
                    policy,
                    CancellationToken::new(),
                )
                .await
                .expect("bwrap spawn");
            let mut exit = None;
            let mut output = Vec::new();
            while let Some(event) = process.events.recv().await {
                match event {
                    ProcessEvent::Stdout(chunk) | ProcessEvent::Stderr(chunk) => {
                        output.extend(chunk)
                    }
                    ProcessEvent::Exit { code, .. } => {
                        exit = code;
                        break;
                    }
                }
            }
            (exit, output)
        }

        let (allowed_exit, allowed_output) =
            run_cat(&backend, policy.clone(), &workspace, cat, &allowed).await;
        assert_eq!(
            allowed_exit,
            Some(0),
            "{}",
            String::from_utf8_lossy(&allowed_output)
        );
        assert!(allowed_output
            .windows(b"bwrap-allowed-canary".len())
            .any(|window| window == b"bwrap-allowed-canary"));

        let (outside_exit, outside_output) =
            run_cat(&backend, policy, &workspace, cat, &outside).await;
        assert_ne!(outside_exit, Some(0));
        assert!(!outside_output
            .windows(b"bwrap-outside-secret".len())
            .any(|window| window == b"bwrap-outside-secret"));
    }
}
