//! macOS sandbox-exec（Seatbelt）硬隔离后端。
//!
//! 纯函数 [`escape_seatbelt_string`] / [`generate_seatbelt_profile`] 在所有平台编译
//! 并单测；spawn 后端 [`SandboxExecBackend`] 与探测仅在 macOS 编译。

use std::path::PathBuf;

use crate::sandbox::{NetworkMode, SandboxPolicy};

/// sandbox-exec 二进制路径（macOS 固定位置）。
pub const SANDBOX_EXEC_PATH: &str = "/usr/bin/sandbox-exec";

/// 转义 Seatbelt profile 字符串字面量：双引号包裹，转义 `\` 与 `"`。
pub fn escape_seatbelt_string(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 2);
    out.push('"');
    for ch in input.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 把 [`SandboxPolicy`] 编译为 Seatbelt profile 文本（version 1 s-expression）。
///
/// 强项：文件系统 read/write/deny 与网络 Enforce。`max_procs` 在 Seatbelt 中无可靠
/// 原语，profile 仅以注释标注，实际限制依赖 RLIMIT_NPROC（诚实降级）。
pub fn generate_seatbelt_profile(policy: &SandboxPolicy, workspace_roots: &[PathBuf]) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(s, "(version 1)");
    let _ = writeln!(s, "(deny default)");
    let _ = writeln!(s, "(allow process-exec)");
    let _ = writeln!(s, "(allow process-fork)");
    let _ = writeln!(s, "(allow signal (target self))");
    let _ = writeln!(s, "(allow sysctl-read)");
    let _ = writeln!(s, "(allow mach-lookup)");
    let _ = writeln!(s, "(allow ipc-posix-shm)");
    let _ = writeln!(s, "(allow file-read-metadata)");
    for sys in [
        "/bin",
        "/private/etc",
        "/private/var/db/dyld",
        "/System",
        "/usr/bin",
        "/usr/lib",
        "/usr/share",
        "/Library/Apple",
        "/Library/Frameworks",
        "/System/Library/Frameworks",
    ] {
        let _ = writeln!(
            s,
            "(allow file-read* (subpath {}))",
            escape_seatbelt_string(sys)
        );
    }
    // Darwin 25+：firmlink / cryptex 使上面的 system subpath 不足以加载
    // `/bin/echo`（SIGABRT 134）。读放开到 `/`，写仍仅 write_roots。
    // 随后的 secret deny 必须用 `file-read*`（`file*` 盖不住这条 allow）。
    let _ = writeln!(s, "(allow file-read* (subpath \"/\"))");
    for r in &policy.filesystem.read_roots {
        let _ = writeln!(
            s,
            "(allow file-read* (subpath {}))",
            escape_seatbelt_string(&r.to_string_lossy())
        );
    }
    for w in &policy.filesystem.write_roots {
        let _ = writeln!(
            s,
            "(allow file-read* (subpath {}))",
            escape_seatbelt_string(&w.to_string_lossy())
        );
        let _ = writeln!(
            s,
            "(allow file-write* (subpath {}))",
            escape_seatbelt_string(&w.to_string_lossy())
        );
    }
    for root in workspace_roots {
        let _ = writeln!(
            s,
            "(allow file-read* (subpath {}))",
            escape_seatbelt_string(&root.to_string_lossy())
        );
    }
    for d in &policy.filesystem.deny {
        let mut denied = vec![d.clone()];
        if let Ok(canon) = std::fs::canonicalize(d) {
            if !denied.iter().any(|p| p == &canon) {
                denied.push(canon);
            }
        }
        for path in denied {
            let escaped = escape_seatbelt_string(&path.to_string_lossy());
            let _ = writeln!(s, "(deny file-read* (subpath {escaped}))");
            let _ = writeln!(s, "(deny file-write* (subpath {escaped}))");
        }
    }
    match policy.network_mode {
        NetworkMode::Enforce => {
            let _ = writeln!(s, "(deny network*)");
            // not implemented, awaiting egress broker: network_allow_hosts 仅记录意图，
            // 不编译进 profile（Seatbelt 需解析后的 endpoint filter），Enforce 下网络保持全拒。
            if !policy.network_allow_hosts.is_empty() {
                let _ = writeln!(
                    s,
                    "; hostname allowlist intentionally not compiled: Seatbelt requires resolved endpoint filters; network remains denied"
                );
            }
        }
        NetworkMode::Hint | NetworkMode::Off => {
            let _ = writeln!(s, "(allow network*)");
        }
    }
    if let Some(max) = policy.max_procs {
        let _ = writeln!(
            s,
            "; max_procs={max}: not enforceable by Seatbelt; rely on RLIMIT_NPROC"
        );
    }
    s
}

#[cfg(target_os = "macos")]
mod seatbelt {
    use super::{generate_seatbelt_profile, SANDBOX_EXEC_PATH};
    use crate::cancel::CancellationToken;
    use crate::process::ProcessRuntime;
    use crate::sandbox::{
        apply_soft_restrictions, SandboxBackend, SandboxError, SandboxInteractiveProcess,
        SandboxPolicy, SandboxProcess, SandboxProcessSpec,
    };
    use async_trait::async_trait;
    use std::path::Path;
    use std::sync::OnceLock;

    fn run_probe() -> (bool, String) {
        let p = Path::new(SANDBOX_EXEC_PATH);
        if !p.exists() {
            return (false, format!("{SANDBOX_EXEC_PATH} not found"));
        }
        let profile = generate_seatbelt_profile(&SandboxPolicy::default(), &[]);
        match std::process::Command::new(SANDBOX_EXEC_PATH)
            .args(["-p", &profile, "/usr/bin/true"])
            .output()
        {
            Ok(output) if output.status.success() => (true, String::new()),
            Ok(output) => (
                false,
                format!("sandbox-exec smoke probe failed: {}", output.status),
            ),
            Err(error) => (false, format!("sandbox-exec probe failed: {error}")),
        }
    }

    pub(super) fn probe() -> &'static (bool, String) {
        static PROBE: OnceLock<(bool, String)> = OnceLock::new();
        PROBE.get_or_init(run_probe)
    }

    pub fn probe_reason() -> String {
        probe().1.clone()
    }

    /// macOS sandbox-exec（Seatbelt）硬隔离后端。
    #[derive(Clone, Debug)]
    pub struct SandboxExecBackend {
        runtime: ProcessRuntime,
        available: bool,
    }

    impl SandboxExecBackend {
        pub fn with_runtime(runtime: ProcessRuntime) -> Self {
            let available = probe().0;
            Self { runtime, available }
        }
    }

    #[async_trait]
    impl SandboxBackend for SandboxExecBackend {
        fn id(&self) -> &'static str {
            "sandbox_exec"
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
                return Err(SandboxError::BackendUnavailable("sandbox_exec"));
            }
            apply_soft_restrictions(&mut spec, &policy)?;
            let profile = generate_seatbelt_profile(&policy, &spec.workspace_roots);
            let inner_program = spec.command.program.clone();
            let inner_args = std::mem::take(&mut spec.command.args);
            let mut argv: Vec<String> = Vec::with_capacity(inner_args.len() + 3);
            argv.push("-p".into());
            argv.push(profile);
            argv.push(inner_program);
            argv.extend(inner_args);
            spec.command.program = SANDBOX_EXEC_PATH.to_string();
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
                return Err(SandboxError::BackendUnavailable("sandbox_exec"));
            }
            apply_soft_restrictions(&mut spec, &policy)?;
            let profile = generate_seatbelt_profile(&policy, &spec.workspace_roots);
            let inner_program = spec.command.program.clone();
            let inner_args = std::mem::take(&mut spec.command.args);
            let mut argv: Vec<String> = Vec::with_capacity(inner_args.len() + 3);
            argv.push("-p".into());
            argv.push(profile);
            argv.push(inner_program);
            argv.extend(inner_args);
            spec.command.program = SANDBOX_EXEC_PATH.to_string();
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

#[cfg(target_os = "macos")]
pub use seatbelt::{probe_reason, SandboxExecBackend};

#[cfg(not(target_os = "macos"))]
pub fn probe_reason() -> String {
    "sandbox-exec only available on macOS".to_string()
}

/// macOS 进程树终止：用 `proc_listpids` + `proc_pidinfo` 实现与
/// [`crate::os::linux::linux_process_tree::terminate`] 同语义——冻树、扫 ppid
/// 链、按 start_time 防 PID 复用、`killpg` + 杀已 `setsid` 逃逸的后代。
#[cfg(target_os = "macos")]
pub(crate) mod macos_process_tree {
    use std::collections::{HashMap, HashSet};
    use std::io;

    const MAX_FREEZE_ROUNDS: usize = 16;
    /// `<libproc.h>` `PROC_ALL_PIDS`；libc 未导出该常量。
    const PROC_ALL_PIDS: u32 = 1;

    #[derive(Clone, Copy, Debug)]
    struct ProcessRecord {
        pid: i32,
        ppid: i32,
        pgrp: i32,
        start_time: u64,
    }

    fn encode_start_time(sec: u64, usec: u64) -> u64 {
        sec.saturating_mul(1_000_000).saturating_add(usec)
    }

    fn read_process(pid: i32) -> io::Result<Option<ProcessRecord>> {
        let mut info = unsafe { std::mem::zeroed::<libc::proc_bsdinfo>() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: `info` 是本函数栈上的 `proc_bsdinfo`；`proc_pidinfo` 至多写入
        // `size` 字节。失败（进程已退出 / 无权限）按 Linux `/proc` 缺席处理。
        let got = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&raw mut info).cast(),
                size,
            )
        };
        if got <= 0 || got != size {
            return Ok(None);
        }
        let Ok(pid) = i32::try_from(info.pbi_pid) else {
            return Ok(None);
        };
        let Ok(ppid) = i32::try_from(info.pbi_ppid) else {
            return Ok(None);
        };
        let Ok(pgrp) = i32::try_from(info.pbi_pgid) else {
            return Ok(None);
        };
        Ok(Some(ProcessRecord {
            pid,
            ppid,
            pgrp,
            start_time: encode_start_time(info.pbi_start_tvsec, info.pbi_start_tvusec),
        }))
    }

    fn snapshot() -> io::Result<HashMap<i32, ProcessRecord>> {
        // SAFETY: buffer=NULL 且 buffersize=0 是 `proc_listpids` 的询大小约定，不写内存。
        let needed = unsafe { libc::proc_listpids(PROC_ALL_PIDS, 0, std::ptr::null_mut(), 0) };
        if needed <= 0 {
            return Err(io::Error::last_os_error());
        }
        let slack = 128 * std::mem::size_of::<libc::pid_t>();
        let mut bytes = vec![0u8; needed as usize + slack];
        // SAFETY: `bytes` 是本函数拥有的缓冲区；长度以字节传给 `proc_listpids`。
        let filled = unsafe {
            libc::proc_listpids(
                PROC_ALL_PIDS,
                0,
                bytes.as_mut_ptr().cast(),
                bytes.len() as libc::c_int,
            )
        };
        if filled <= 0 {
            return Err(io::Error::last_os_error());
        }
        let pid_size = std::mem::size_of::<libc::pid_t>();
        let n = ((filled as usize) / pid_size).min(bytes.len() / pid_size);
        let pids = unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<libc::pid_t>(), n) };
        let mut processes = HashMap::new();
        for &raw in pids {
            if raw <= 0 {
                continue;
            }
            if let Some(process) = read_process(raw)? {
                processes.insert(process.pid, process);
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
        fn start_time_reads_current_process() {
            let pid = i32::try_from(std::process::id()).expect("pid fits i32");
            let started = start_time(pid).expect("self start_time");
            assert!(started > 0);
            let snapshot = snapshot().expect("snapshot");
            let self_record = snapshot.get(&pid).expect("self pid in snapshot");
            assert_eq!(self_record.start_time, started);
            assert_eq!(self_record.pid, pid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::FilesystemPolicy;

    #[test]
    fn seatbelt_escape_quotes_and_backslashes() {
        assert_eq!(escape_seatbelt_string("/a/b"), "\"/a/b\"");
        assert_eq!(escape_seatbelt_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }

    #[test]
    fn profile_denies_network_when_enforce() {
        let policy = SandboxPolicy {
            network_mode: NetworkMode::Enforce,
            ..Default::default()
        };
        let profile = generate_seatbelt_profile(&policy, &[]);
        assert!(profile.contains("(deny network*)"));
    }

    #[test]
    fn profile_allows_network_when_hint() {
        let policy = SandboxPolicy {
            network_mode: NetworkMode::Hint,
            ..Default::default()
        };
        let profile = generate_seatbelt_profile(&policy, &[]);
        assert!(profile.contains("(allow network*)"));
        assert!(!profile.contains("(deny network*)"));
    }

    #[test]
    fn profile_emits_deny_for_secret_paths() {
        let deny = vec![
            PathBuf::from("/Users/x/.ssh"),
            PathBuf::from("/Users/x/.pawork/auth.json"),
            PathBuf::from("/opt/pawork-home/auth.json"),
            PathBuf::from("/Users/x/.gnupg"),
            PathBuf::from("/Users/x/.config"),
        ];
        let policy = SandboxPolicy {
            filesystem: FilesystemPolicy {
                deny: deny.clone(),
                ..Default::default()
            },
            ..Default::default()
        };
        let profile = generate_seatbelt_profile(&policy, &[]);
        assert!(
            profile.contains("(allow file-read* (subpath \"/\"))"),
            "Darwin 25+ 整盘只读 allow 必须保留"
        );
        for path in &deny {
            let escaped = escape_seatbelt_string(&path.to_string_lossy());
            assert!(
                profile.contains(&format!("(deny file-read* (subpath {escaped}))")),
                "profile 缺少 file-read deny: {escaped}"
            );
            assert!(
                profile.contains(&format!("(deny file-write* (subpath {escaped}))")),
                "profile 缺少 file-write deny: {escaped}"
            );
        }
    }

    #[test]
    fn profile_emits_deny_for_default_secret_paths() {
        let secrets = crate::sandbox::default_secret_paths();
        if secrets.is_empty() {
            return;
        }
        let policy = SandboxPolicy {
            filesystem: FilesystemPolicy {
                deny: secrets.clone(),
                ..Default::default()
            },
            ..Default::default()
        };
        let profile = generate_seatbelt_profile(&policy, &[]);
        assert!(profile.contains("(allow file-read* (subpath \"/\"))"));
        for path in &secrets {
            let escaped = escape_seatbelt_string(&path.to_string_lossy());
            assert!(
                profile.contains(&format!("(deny file-read* (subpath {escaped}))")),
                "default secret 未写入 deny: {escaped}"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn seatbelt_denies_cat_of_secret_paths() {
        let probe_profile = generate_seatbelt_profile(&SandboxPolicy::default(), &[]);
        let probe = std::process::Command::new(SANDBOX_EXEC_PATH)
            .args(["-p", &probe_profile, "/usr/bin/true"])
            .output();
        if !matches!(&probe, Ok(output) if output.status.success()) {
            return;
        }

        let tmp = std::env::temp_dir().join(format!(
            "pawork-seatbelt-secret-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).expect("create secret dir");
        let secret = tmp.join("auth.json");
        std::fs::write(&secret, "secret-canary").expect("write secret");

        let policy = SandboxPolicy {
            filesystem: FilesystemPolicy {
                deny: vec![tmp.clone()],
                ..Default::default()
            },
            network_mode: NetworkMode::Off,
            ..Default::default()
        };
        let profile = generate_seatbelt_profile(&policy, &[]);
        assert!(profile.contains("(allow file-read* (subpath \"/\"))"));
        let output = std::process::Command::new(SANDBOX_EXEC_PATH)
            .args(["-p", &profile, "/bin/cat", &secret.to_string_lossy()])
            .output()
            .expect("sandbox-exec cat");
        assert!(
            !output.status.success(),
            "cat of denied secret path must fail under Seatbelt: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !stdout.contains("secret-canary"),
            "denied cat must not leak secret contents: {stdout}"
        );

        let default_policy = SandboxPolicy {
            filesystem: FilesystemPolicy {
                deny: crate::sandbox::default_secret_paths(),
                ..Default::default()
            },
            network_mode: NetworkMode::Off,
            ..Default::default()
        };
        let default_profile = generate_seatbelt_profile(&default_policy, &[]);
        for path in crate::sandbox::default_secret_paths() {
            let target = if path.is_file() {
                path
            } else if path.is_dir() {
                match first_regular_file(&path) {
                    Some(file) => file,
                    None => continue,
                }
            } else {
                continue;
            };
            let output = std::process::Command::new(SANDBOX_EXEC_PATH)
                .args(["-p", &default_profile, "/bin/cat", &target.to_string_lossy()])
                .output()
                .expect("sandbox-exec cat default secret");
            assert!(
                !output.status.success(),
                "cat {} must be denied by default secret paths",
                target.display()
            );
        }

        let _ = std::fs::remove_dir_all(tmp);
    }

    #[cfg(target_os = "macos")]
    fn first_regular_file(dir: &std::path::Path) -> Option<PathBuf> {
        let entries = std::fs::read_dir(dir).ok()?;
        entries.flatten().map(|entry| entry.path()).find(|p| p.is_file())
    }

    #[test]
    fn profile_emits_canonical_deny_for_existing_path() {
        let tmp = std::env::temp_dir().join(format!(
            "pawork-seatbelt-canon-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).expect("create deny dir");
        let policy = SandboxPolicy {
            filesystem: FilesystemPolicy {
                deny: vec![tmp.clone()],
                ..Default::default()
            },
            ..Default::default()
        };
        let profile = generate_seatbelt_profile(&policy, &[]);
        let logical = escape_seatbelt_string(&tmp.to_string_lossy());
        assert!(profile.contains(&format!("(deny file-read* (subpath {logical}))")));
        if let Ok(canon) = std::fs::canonicalize(&tmp) {
            let escaped = escape_seatbelt_string(&canon.to_string_lossy());
            assert!(
                profile.contains(&format!("(deny file-read* (subpath {escaped}))")),
                "existing deny path must also emit canonical form: {escaped}"
            );
        }
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn profile_emits_write_roots_as_file_write() {
        let policy = SandboxPolicy {
            filesystem: FilesystemPolicy {
                write_roots: vec![PathBuf::from("/tmp/ws")],
                ..Default::default()
            },
            ..Default::default()
        };
        let profile = generate_seatbelt_profile(&policy, &[]);
        assert!(profile.contains("(allow file-write* (subpath \"/tmp/ws\"))"));
    }

    #[test]
    fn profile_notes_max_procs_unenforced() {
        let policy = SandboxPolicy {
            max_procs: Some(8),
            ..Default::default()
        };
        let profile = generate_seatbelt_profile(&policy, &[]);
        assert!(profile.contains("max_procs=8"));
        assert!(profile.contains("RLIMIT_NPROC"));
    }

    #[test]
    fn profile_includes_version_header() {
        let profile = generate_seatbelt_profile(&SandboxPolicy::default(), &[]);
        assert!(profile.contains("(version 1)"));
        assert!(profile.contains("(deny default)"));
    }
}
