//! 跨平台 Process Runtime（P4-12 / P11-7）。
//!
//! 进程组（Unix）/ 递归终止（Windows）；stdout/stderr 无死锁并发读取；
//! 超大输出截断；timeout 与协作式 cancel 任一触发即终止进程树。
//!
//! - Unix：`pre_exec` 中 `setpgid(0,0)` 建立进程组，kill 树用 `killpg(pgid)`；
//!   Linux 额外冻结并遍历 `/proc` 后代，回收已通过 `setsid` 离组的进程。
//! - Windows：Job Object 绑定整棵进程树，句柄关闭即回收后代。

use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_domain::CancellationToken;
use tokio::io::AsyncReadExt;
use tokio::io::BufReader;
use tokio::process::Child;
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

const PROCESS_TREE_KILL_TIMEOUT: Duration = Duration::from_secs(5);

/// 由操作系统施加的进程资源上限。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessLimits {
    pub cpu_time: Option<Duration>,
    pub memory_bytes: Option<u64>,
    pub open_files: Option<u64>,
    pub max_processes: Option<u32>,
}

/// Linux Landlock 文件系统白名单。
///
/// 路径在父进程中解析并转换为 ruleset FD；子进程的 `pre_exec` 只执行
/// `PR_SET_NO_NEW_PRIVS` 与 `landlock_restrict_self(2)`，避免 fork 后打开文件。
#[cfg(target_os = "linux")]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinuxLandlockPolicy {
    pub read_paths: Vec<std::path::PathBuf>,
    pub write_paths: Vec<std::path::PathBuf>,
}

/// 子进程规格。
#[derive(Clone, Debug)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<std::path::PathBuf>,
    pub env_clear: bool,
    pub env: Vec<(String, String)>,
    pub timeout: Option<Duration>,
    pub max_output_bytes: u64,
    pub limits: ProcessLimits,
    #[cfg(target_os = "linux")]
    pub landlock: Option<LinuxLandlockPolicy>,
}

impl CommandSpec {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env_clear: false,
            env: Vec::new(),
            timeout: None,
            max_output_bytes: 8 * 1024 * 1024,
            limits: ProcessLimits::default(),
            #[cfg(target_os = "linux")]
            landlock: None,
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }
}

/// 缓冲执行结果。
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
    pub truncated: bool,
    pub timed_out: bool,
    pub killed: bool,
}

/// 流式事件。
#[derive(Clone, Debug)]
pub enum ProcessEvent {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Exit { code: Option<i32>, truncated: bool },
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("failed to spawn process `{program}`: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to attach process `{program}` to its process-tree guard: {source}")]
    ProcessTree {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to prepare process isolation for `{program}`: {source}")]
    Isolation {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("timed out waiting for process tree {process_id:?} to terminate")]
    KillTimeout { process_id: Option<u32> },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// 进程树句柄：向持有 child 的监督任务发出终止信号。
pub struct ProcessHandle {
    process_id: Option<u32>,
    kill: CancellationToken,
    done: watch::Receiver<bool>,
}

impl ProcessHandle {
    /// 终止整个进程树。幂等，且等待时间有界。
    pub async fn kill(&mut self) -> Result<(), ProcessError> {
        if *self.done.borrow() {
            return Ok(());
        }
        self.kill.cancel();
        let process_id = self.process_id;
        tokio::time::timeout(PROCESS_TREE_KILL_TIMEOUT, async {
            while !*self.done.borrow() {
                if self.done.changed().await.is_err() {
                    break;
                }
            }
        })
        .await
        .map_err(|_| ProcessError::KillTimeout { process_id })
    }

    pub fn id(&self) -> Option<u32> {
        self.process_id
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        if !*self.done.borrow() {
            self.kill.cancel();
        }
    }
}

/// Process Runtime：跨平台子进程执行。
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessRuntime;

impl ProcessRuntime {
    pub fn new() -> Self {
        Self
    }

    /// 缓冲执行：捕获 stdout/stderr，应用 timeout 与 max_output_bytes。
    pub async fn run(
        &self,
        spec: CommandSpec,
        cancel: CancellationToken,
    ) -> Result<ProcessOutput, ProcessError> {
        let (mut child, tree) = spawn_child(&spec)?;
        let max = spec.max_output_bytes;

        // 并发读 stdout/stderr，避免管道死锁。
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let stdout_task = tokio::spawn(async move {
            stdout
                .map(|s| {
                    let fut = async {
                        let mut b = BufReader::new(s);
                        collect_to_vec(&mut b, max).await
                    };
                    Box::pin(fut)
                        as std::pin::Pin<
                            Box<dyn std::future::Future<Output = (Vec<u8>, bool)> + Send>,
                        >
                })
                .unwrap_or_else(|| Box::pin(async { (Vec::new(), false) }))
                .await
        });
        let stderr_task = tokio::spawn(async move {
            stderr
                .map(|s| {
                    let fut = async {
                        let mut b = BufReader::new(s);
                        collect_to_vec(&mut b, max).await
                    };
                    Box::pin(fut)
                        as std::pin::Pin<
                            Box<dyn std::future::Future<Output = (Vec<u8>, bool)> + Send>,
                        >
                })
                .unwrap_or_else(|| Box::pin(async { (Vec::new(), false) }))
                .await
        });

        // 等待退出 / timeout / cancel。
        let mut timed_out = false;
        let mut killed = false;
        let exit_code = if let Some(dur) = spec.timeout {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    kill_child_tree(&mut child, &tree).await;
                    killed = true;
                    None
                }
                _ = tokio::time::sleep(dur) => {
                    kill_child_tree(&mut child, &tree).await;
                    timed_out = true;
                    None
                }
                status = child.wait() => {
                    let code = status.ok().and_then(|st| st.code());
                    let _ = tree.terminate();
                    code
                }
            }
        } else {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    kill_child_tree(&mut child, &tree).await;
                    killed = true;
                    None
                }
                status = child.wait() => {
                    let code = status.ok().and_then(|st| st.code());
                    let _ = tree.terminate();
                    code
                }
            }
        };

        let stdout_out = stdout_task.await.unwrap_or_default();
        let stderr_out = stderr_task.await.unwrap_or_default();
        let truncated = stdout_out.1 || stderr_out.1;

        Ok(ProcessOutput {
            stdout: stdout_out.0,
            stderr: stderr_out.0,
            exit_code,
            truncated,
            timed_out,
            killed,
        })
    }

    /// 流式执行：返回事件接收器与进程句柄。
    pub async fn spawn_stream(
        &self,
        spec: CommandSpec,
        cancel: CancellationToken,
    ) -> Result<(mpsc::Receiver<ProcessEvent>, ProcessHandle), ProcessError> {
        let (mut child, tree) = spawn_child(&spec)?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (tx, rx) = mpsc::channel(64);
        let remaining = Arc::new(AtomicU64::new(spec.max_output_bytes));
        let truncated = Arc::new(AtomicBool::new(false));

        let stdout_task = stdout.map(|stdout| {
            let tx = tx.clone();
            let remaining = remaining.clone();
            let truncated = truncated.clone();
            tokio::spawn(stream_chunks(
                stdout,
                ProcessEvent::Stdout,
                tx,
                remaining,
                truncated,
            ))
        });
        let stderr_task = stderr.map(|stderr| {
            let tx = tx.clone();
            let remaining = remaining.clone();
            let truncated = truncated.clone();
            tokio::spawn(stream_chunks(
                stderr,
                ProcessEvent::Stderr,
                tx,
                remaining,
                truncated,
            ))
        });
        let process_id = child.id();
        let kill = CancellationToken::new();
        let kill_for_task = kill.clone();
        let (done_tx, done) = watch::channel(false);
        let cancel_clone = cancel.clone();
        let timeout = spec.timeout;
        tokio::spawn(async move {
            let code = if let Some(duration) = timeout {
                tokio::select! {
                    biased;
                    _ = cancel_clone.cancelled() => {
                        kill_child_tree(&mut child, &tree).await;
                        None
                    }
                    _ = kill_for_task.cancelled() => {
                        kill_child_tree(&mut child, &tree).await;
                        None
                    }
                    _ = tokio::time::sleep(duration) => {
                        kill_child_tree(&mut child, &tree).await;
                        None
                    }
                    status = child.wait() => {
                        let code = status.ok().and_then(|st| st.code());
                        let _ = tree.terminate();
                        code
                    }
                }
            } else {
                tokio::select! {
                    biased;
                    _ = cancel_clone.cancelled() => {
                        kill_child_tree(&mut child, &tree).await;
                        None
                    }
                    _ = kill_for_task.cancelled() => {
                        kill_child_tree(&mut child, &tree).await;
                        None
                    }
                    status = child.wait() => {
                        let code = status.ok().and_then(|st| st.code());
                        let _ = tree.terminate();
                        code
                    }
                }
            };
            // 句柄等待的是进程树退出，不应被未消费的输出通道背压阻塞。
            let _ = done_tx.send(true);
            if let Some(task) = stdout_task {
                let _ = task.await;
            }
            if let Some(task) = stderr_task {
                let _ = task.await;
            }
            let _ = tx
                .send(ProcessEvent::Exit {
                    code,
                    truncated: truncated.load(Ordering::Acquire),
                })
                .await;
        });

        let handle = ProcessHandle {
            process_id,
            kill,
            done,
        };
        Ok((rx, handle))
    }
}

/// spawn 子进程并立即绑定平台进程树守卫。
fn spawn_child(spec: &CommandSpec) -> Result<(Child, ProcessTreeGuard), ProcessError> {
    let mut command = Command::new(&spec.program);
    command.args(&spec.args);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.kill_on_drop(true);
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    if spec.env_clear {
        command.env_clear();
    }
    for (k, v) in &spec.env {
        command.env(k, v);
    }
    #[cfg(windows)]
    {
        // 先挂起创建，绑定 Job Object 后再恢复，关闭子进程在绑定前逃逸后代的竞态。
        command.creation_flags(windows::Win32::System::Threading::CREATE_SUSPENDED.0);
    }
    #[cfg(unix)]
    {
        // SAFETY: pre_exec 内只调用 libc 资源/进程 API 与预先创建的 Landlock FD。
        unsafe { configure_unix_child(&mut command, spec) }.map_err(|source| {
            ProcessError::Isolation {
                program: spec.program.clone(),
                source,
            }
        })?;
    }
    let mut child = command.spawn().map_err(|source| ProcessError::Spawn {
        program: spec.program.clone(),
        source,
    })?;
    match ProcessTreeGuard::attach(&child, spec.limits) {
        Ok(tree) => {
            #[cfg(windows)]
            if let Err(source) = windows_job::resume(&child) {
                let _ = tree.terminate();
                let _ = child.start_kill();
                return Err(ProcessError::ProcessTree {
                    program: spec.program.clone(),
                    source,
                });
            }
            Ok((child, tree))
        }
        Err(source) => {
            let _ = child.start_kill();
            Err(ProcessError::ProcessTree {
                program: spec.program.clone(),
                source,
            })
        }
    }
}

#[cfg(unix)]
unsafe fn configure_unix_child(command: &mut Command, spec: &CommandSpec) -> std::io::Result<()> {
    let limits = spec.limits;
    #[cfg(target_os = "linux")]
    let landlock_fd = spec
        .landlock
        .as_ref()
        .map(prepare_linux_landlock)
        .transpose()?;

    command.pre_exec(move || {
        if libc::setpgid(0, 0) == -1 {
            return Err(std::io::Error::last_os_error());
        }

        macro_rules! set_limit {
            ($resource:expr, $value:expr) => {{
                let value = ($value).min(libc::rlim_t::MAX as u64) as libc::rlim_t;
                let limit = libc::rlimit {
                    rlim_cur: value,
                    rlim_max: value,
                };
                if libc::setrlimit($resource, &limit) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
            }};
        }

        if let Some(cpu_time) = limits.cpu_time {
            set_limit!(libc::RLIMIT_CPU, cpu_time.as_secs().max(1));
        }
        if let Some(memory_bytes) = limits.memory_bytes {
            set_limit!(libc::RLIMIT_AS, memory_bytes);
        }
        if let Some(open_files) = limits.open_files {
            set_limit!(libc::RLIMIT_NOFILE, open_files);
        }
        if let Some(max_processes) = limits.max_processes {
            set_limit!(libc::RLIMIT_NPROC, u64::from(max_processes));
        }

        #[cfg(target_os = "linux")]
        {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            // 关闭 fork 与设置 PDEATHSIG 之间的父进程死亡竞态。
            if libc::getppid() == 1 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "parent exited before child initialization",
                ));
            }
            if let Some(ruleset_fd) = landlock_fd.as_ref() {
                use std::os::fd::AsRawFd;

                restrict_linux_landlock(ruleset_fd.as_raw_fd())?;
            }
        }
        Ok(())
    });
    Ok(())
}

/// 探测当前 Linux 内核是否能创建至少一个 Landlock 文件系统 ruleset。
#[cfg(target_os = "linux")]
pub fn linux_landlock_supported() -> Result<(), String> {
    std::thread::Builder::new()
        .name("pawork-landlock-probe".into())
        .spawn(|| {
            use std::os::fd::AsRawFd;

            let ruleset = prepare_linux_landlock(&LinuxLandlockPolicy::default())?;
            // Landlock 默认只限制调用线程；探测线程随即退出，不影响调用方其他线程。
            restrict_linux_landlock(ruleset.as_raw_fd())
        })
        .map_err(|error| error.to_string())?
        .join()
        .map_err(|_| "Landlock probe thread panicked".to_string())?
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "linux")]
fn restrict_linux_landlock(ruleset_fd: std::os::fd::RawFd) -> std::io::Result<()> {
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset_fd, 0) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn prepare_linux_landlock(policy: &LinuxLandlockPolicy) -> std::io::Result<std::os::fd::OwnedFd> {
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
mod linux_process_tree {
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

    pub(super) fn start_time(pid: i32) -> io::Result<u64> {
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

    pub(super) fn terminate(root_pid: i32, pgid: i32, root_start_time: u64) -> io::Result<()> {
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

/// OS 进程树生命周期守卫。除 `ProcessRuntime` 自身外，PTY 等已取得 PID 的宿主
/// 也可通过 [`attach_external`](Self::attach_external) 复用同一终止契约。
pub struct ProcessTreeGuard {
    #[cfg(unix)]
    pgid: i32,
    #[cfg(target_os = "linux")]
    root_pid: i32,
    #[cfg(target_os = "linux")]
    root_start_time: u64,
    #[cfg(windows)]
    job: windows_job::Job,
}

impl ProcessTreeGuard {
    /// 绑定由其他进程启动器创建的子进程。
    ///
    /// Unix 要求目标已经是自己的 process-group leader（PTY 子进程经 `setsid` 满足）；
    /// Windows 为目标创建并绑定带 `KILL_ON_JOB_CLOSE` 的 Job Object。
    pub fn attach_external(process_id: u32, limits: ProcessLimits) -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            let _ = limits;
            let pid = i32::try_from(process_id).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "process id exceeds i32")
            })?;
            let pgid = unsafe { libc::getpgid(pid) };
            if pgid == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if pgid != pid {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("external process {pid} is not a process-group leader (pgid={pgid})"),
                ));
            }
            #[cfg(target_os = "linux")]
            let root_start_time = linux_process_tree::start_time(pid)?;
            Ok(Self {
                pgid,
                #[cfg(target_os = "linux")]
                root_pid: pid,
                #[cfg(target_os = "linux")]
                root_start_time,
            })
        }
        #[cfg(windows)]
        {
            Ok(Self {
                job: windows_job::Job::attach_pid(process_id, limits)?,
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (process_id, limits);
            Ok(Self {})
        }
    }

    fn attach(child: &Child, limits: ProcessLimits) -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            let _ = limits;
            let pid = child.id().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "child has no process id")
            })?;
            let pgid = i32::try_from(pid).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "process id exceeds i32")
            })?;
            #[cfg(target_os = "linux")]
            let root_start_time = linux_process_tree::start_time(pgid)?;
            Ok(Self {
                pgid,
                #[cfg(target_os = "linux")]
                root_pid: pgid,
                #[cfg(target_os = "linux")]
                root_start_time,
            })
        }
        #[cfg(windows)]
        {
            Ok(Self {
                job: windows_job::Job::attach(child, limits)?,
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (child, limits);
            Ok(Self {})
        }
    }

    /// 终止守卫覆盖的完整进程树。该操作幂等。
    pub fn terminate(&self) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            #[cfg(target_os = "linux")]
            {
                linux_process_tree::terminate(self.root_pid, self.pgid, self.root_start_time)
            }
            #[cfg(not(target_os = "linux"))]
            {
                let result = unsafe { libc::killpg(self.pgid, libc::SIGKILL) };
                if result == 0 {
                    return Ok(());
                }
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ESRCH) {
                    Ok(())
                } else {
                    Err(error)
                }
            }
        }
        #[cfg(windows)]
        {
            self.job.terminate()
        }
        #[cfg(not(any(unix, windows)))]
        {
            Ok(())
        }
    }
}

/// 终止整个进程树，并在固定时限内回收直接子进程。
async fn kill_child_tree(child: &mut Child, tree: &ProcessTreeGuard) {
    let _ = tree.terminate();
    let _ = child.start_kill();
    let _ = tokio::time::timeout(PROCESS_TREE_KILL_TIMEOUT, child.wait()).await;
}

#[cfg(windows)]
mod windows_job {
    use std::collections::{HashMap, HashSet};
    use std::io;
    use std::mem::size_of;

    use tokio::process::Child;
    use windows::Win32::Foundation::{CloseHandle, BOOL, FILETIME, HANDLE, STILL_ACTIVE};
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

    use super::ProcessLimits;

    #[link(name = "ntdll")]
    extern "system" {
        fn NtResumeProcess(process_handle: HANDLE) -> i32;
    }

    /// 独占拥有的 Job Object；Drop 关闭句柄并触发 KILL_ON_JOB_CLOSE。
    #[derive(Debug)]
    pub(super) struct Job(HANDLE);

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

        pub(super) fn attach(child: &Child, limits: ProcessLimits) -> io::Result<Self> {
            let job = Self::create(limits)?;

            let process = HANDLE(child.raw_handle().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "child has no process handle")
            })?);
            unsafe { AssignProcessToJobObject(job.0, process) }.map_err(io::Error::other)?;
            Ok(job)
        }

        pub(super) fn attach_pid(process_id: u32, limits: ProcessLimits) -> io::Result<Self> {
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
                    IsProcessInJob(process, self.0, &mut in_job).is_ok() && in_job.as_bool()
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
            let result = unsafe { IsProcessInJob(process, self.0, &mut in_job) };
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

        pub(super) fn terminate(&self) -> io::Result<()> {
            unsafe { TerminateJobObject(self.0, 1) }.map_err(io::Error::other)
        }
    }

    pub(super) fn resume(child: &Child) -> io::Result<()> {
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

/// 收集流直到达到字节上限，返回 (内容, 是否截断)。
async fn collect_to_vec<R>(buf: &mut BufReader<R>, max: u64) -> (Vec<u8>, bool)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut out = Vec::new();
    let mut truncated = false;
    let mut chunk = [0u8; 8192];
    loop {
        match buf.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                if out.len() as u64 + n as u64 > max {
                    let remaining = (max as usize).saturating_sub(out.len());
                    out.extend_from_slice(&chunk[..remaining.min(n)]);
                    truncated = true;
                    break;
                }
                out.extend_from_slice(&chunk[..n]);
            }
            Err(_) => break,
        }
    }
    (out, truncated)
}

/// 流式按块读取并发送事件。
async fn stream_chunks<R, F>(
    reader: R,
    make_event: F,
    tx: mpsc::Sender<ProcessEvent>,
    remaining: Arc<AtomicU64>,
    truncated: Arc<AtomicBool>,
) where
    R: tokio::io::AsyncRead + Unpin,
    F: Fn(Vec<u8>) -> ProcessEvent + Send + Sync + 'static,
{
    let mut buf = BufReader::new(reader);
    let mut chunk = vec![0u8; 8192];
    loop {
        match buf.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                let allowed = reserve_output_bytes(&remaining, n);
                if allowed < n {
                    truncated.store(true, Ordering::Release);
                }
                if allowed > 0
                    && tx
                        .send(make_event(chunk[..allowed].to_vec()))
                        .await
                        .is_err()
                {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

fn reserve_output_bytes(remaining: &AtomicU64, requested: usize) -> usize {
    loop {
        let available = remaining.load(Ordering::Acquire);
        if available == 0 {
            return 0;
        }
        let reserved = available.min(requested as u64);
        if remaining
            .compare_exchange_weak(
                available,
                available - reserved,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            return reserved as usize;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 在平台 shell 中执行脚本（unix：`sh -c`；Windows：`cmd /d /s /c`）。
    fn shell(script: &str) -> CommandSpec {
        #[cfg(not(windows))]
        let mut spec = CommandSpec::new("sh").args(["-c".to_string(), script.to_string()]);
        #[cfg(windows)]
        let mut spec = CommandSpec::new("cmd").args([
            "/d".to_string(),
            "/s".to_string(),
            "/c".to_string(),
            script.to_string(),
        ]);
        spec.max_output_bytes = 4 * 1024 * 1024;
        spec
    }

    #[tokio::test]
    async fn captures_stdout_and_stderr() {
        let runtime = ProcessRuntime::new();
        #[cfg(not(windows))]
        let spec = shell("echo hello; echo world >&2");
        #[cfg(windows)]
        // 注意：cmd 的 echo 会保留 `&` 与重定向前的空格/数字，因此这些位置不留空格。
        let spec = shell("echo hello& echo world>&2");
        let out = runtime
            .run(spec, CancellationToken::new())
            .await
            .expect("run");
        // cmd 的行尾为 CRLF。
        #[cfg(not(windows))]
        {
            assert_eq!(out.stdout, b"hello\n");
            assert_eq!(out.stderr, b"world\n");
        }
        #[cfg(windows)]
        {
            assert_eq!(out.stdout, b"hello\r\n");
            assert_eq!(out.stderr, b"world\r\n");
        }
        assert_eq!(out.exit_code, Some(0));
        assert!(!out.timed_out && !out.killed);
    }

    #[tokio::test]
    async fn large_output_is_truncated() {
        let runtime = ProcessRuntime::new();
        #[cfg(not(windows))]
        let mut spec = shell("seq 1000000");
        // Windows 无 seq：用 PowerShell 产生足量输出。
        #[cfg(windows)]
        let mut spec = shell("powershell -NoProfile -Command 1..200000");
        spec.max_output_bytes = 1024;
        let out = runtime
            .run(spec, CancellationToken::new())
            .await
            .expect("run");
        assert!(out.truncated, "应截断");
        assert!(out.stdout.len() <= 1024);
    }

    #[tokio::test]
    async fn timeout_kills_process() {
        let runtime = ProcessRuntime::new();
        #[cfg(not(windows))]
        let mut spec = shell("sleep 30");
        // Windows 无 sleep：ping 约 60s。
        #[cfg(windows)]
        let mut spec = shell("ping -n 61 127.0.0.1");
        spec.timeout = Some(Duration::from_millis(200));
        let out = runtime
            .run(spec, CancellationToken::new())
            .await
            .expect("run");
        assert!(out.timed_out);
    }

    #[tokio::test]
    async fn cancel_terminates_process() {
        let runtime = ProcessRuntime::new();
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(async move {
            #[cfg(not(windows))]
            let mut spec = shell("sleep 30");
            #[cfg(windows)]
            let mut spec = shell("ping -n 61 127.0.0.1");
            spec.timeout = Some(Duration::from_secs(60));
            runtime.run(spec, cancel_clone).await
        });
        tokio::time::sleep(Duration::from_millis(150)).await;
        cancel.cancel();
        let out = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("cancel did not return")
            .expect("join")
            .expect("run");
        assert!(out.killed);
    }

    #[tokio::test]
    async fn stream_events_emitted() {
        let runtime = ProcessRuntime::new();
        #[cfg(not(windows))]
        let mut spec = shell("echo a; echo b >&2");
        #[cfg(windows)]
        let mut spec = shell("echo a & echo b 1>&2");
        spec.timeout = Some(Duration::from_secs(5));
        let (mut rx, _handle) = runtime
            .spawn_stream(spec, CancellationToken::new())
            .await
            .expect("spawn");
        let mut got_stdout = false;
        let mut got_stderr = false;
        let mut got_exit = false;
        while let Some(ev) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .ok()
            .flatten()
        {
            match ev {
                ProcessEvent::Stdout(_) => got_stdout = true,
                ProcessEvent::Stderr(_) => got_stderr = true,
                ProcessEvent::Exit { .. } => {
                    got_exit = true;
                    break;
                }
            }
        }
        assert!(got_stdout);
        assert!(got_stderr);
        assert!(got_exit);
    }

    #[tokio::test]
    async fn stream_handle_can_kill_process() {
        let runtime = ProcessRuntime::new();
        #[cfg(not(windows))]
        let spec = shell("sleep 30");
        #[cfg(windows)]
        let spec = shell("ping -n 61 127.0.0.1");
        let (mut rx, mut handle) = runtime
            .spawn_stream(spec, CancellationToken::new())
            .await
            .expect("spawn");
        assert!(handle.id().is_some());
        tokio::time::timeout(Duration::from_secs(5), handle.kill())
            .await
            .expect("kill timed out")
            .expect("kill");
        let mut saw_exit = false;
        while let Some(event) = rx.recv().await {
            if matches!(event, ProcessEvent::Exit { code: None, .. }) {
                saw_exit = true;
                break;
            }
        }
        assert!(saw_exit);
    }

    #[tokio::test]
    async fn kill_is_idempotent_and_reaps_descendants() {
        let runtime = ProcessRuntime::new();
        #[cfg(not(windows))]
        let spec = shell("sleep 30 & child=$!; echo $child; wait $child");
        #[cfg(windows)]
        let spec = CommandSpec::new("powershell").args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$p=Start-Process -FilePath powershell.exe -ArgumentList '-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 30' -PassThru; [Console]::Out.WriteLine($p.Id); [Console]::Out.Flush(); Wait-Process -Id $p.Id",
        ]);

        let (mut rx, mut handle) = runtime
            .spawn_stream(spec, CancellationToken::new())
            .await
            .expect("spawn tree");
        let descendant = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = rx.recv().await {
                if let ProcessEvent::Stdout(chunk) = event {
                    if let Some(pid) = String::from_utf8_lossy(&chunk)
                        .lines()
                        .find_map(|line| line.trim().parse::<u32>().ok())
                    {
                        return pid;
                    }
                }
            }
            panic!("process exited before reporting descendant pid")
        })
        .await
        .expect("descendant pid timed out");
        assert!(process_exists(descendant), "descendant should be running");

        handle.kill().await.expect("first kill");
        handle.kill().await.expect("idempotent kill");

        let reaped = tokio::time::timeout(Duration::from_secs(3), async {
            while process_exists(descendant) {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
        assert!(reaped.is_ok(), "descendant {descendant} survived tree kill");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn kill_reaps_descendant_that_escaped_with_setsid() {
        if std::process::Command::new("setsid")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            return;
        }

        let runtime = ProcessRuntime::new();
        let spec = shell("setsid sh -c 'echo $$; sleep 30' & escaped=$!; wait $escaped");
        let (mut rx, mut handle) = runtime
            .spawn_stream(spec, CancellationToken::new())
            .await
            .expect("spawn escaped tree");
        let descendant = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = rx.recv().await {
                if let ProcessEvent::Stdout(chunk) = event {
                    if let Some(pid) = String::from_utf8_lossy(&chunk)
                        .lines()
                        .find_map(|line| line.trim().parse::<u32>().ok())
                    {
                        return pid;
                    }
                }
            }
            panic!("process exited before reporting escaped descendant pid")
        })
        .await
        .expect("escaped descendant pid timed out");
        let descendant_pid = i32::try_from(descendant).expect("pid fits i32");
        assert_eq!(
            unsafe { libc::getpgid(descendant_pid) },
            descendant_pid,
            "setsid descendant did not leave the root process group"
        );

        handle.kill().await.expect("kill escaped tree");
        let reaped = tokio::time::timeout(Duration::from_secs(3), async {
            while process_exists(descendant) {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
        assert!(
            reaped.is_ok(),
            "setsid descendant {descendant} survived tree kill"
        );
    }

    #[cfg(unix)]
    fn process_exists(pid: u32) -> bool {
        let pid = match i32::try_from(pid) {
            Ok(pid) => pid,
            Err(_) => return false,
        };
        let result = unsafe { libc::kill(pid, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    #[cfg(windows)]
    fn process_exists(pid: u32) -> bool {
        use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
        else {
            return false;
        };
        let mut exit_code = 0u32;
        let running = unsafe { GetExitCodeProcess(handle, &mut exit_code) }.is_ok()
            && exit_code == STILL_ACTIVE.0 as u32;
        let _ = unsafe { CloseHandle(handle) };
        running
    }

    #[tokio::test]
    async fn stream_output_respects_shared_limit() {
        let runtime = ProcessRuntime::new();
        #[cfg(not(windows))]
        let mut spec = shell("yes x | head -c 8192; yes y | head -c 8192 >&2");
        #[cfg(windows)]
        let mut spec = CommandSpec::new("powershell").args([
            "-NoProfile",
            "-Command",
            "[Console]::Out.Write('x'.PadLeft(8192, 'x')); [Console]::Error.Write('y'.PadLeft(8192, 'y'))",
        ]);
        spec.max_output_bytes = 1024;
        let (mut rx, _handle) = runtime
            .spawn_stream(spec, CancellationToken::new())
            .await
            .expect("spawn");
        let mut bytes = 0usize;
        let mut was_truncated = false;
        while let Some(event) = rx.recv().await {
            match event {
                ProcessEvent::Stdout(chunk) | ProcessEvent::Stderr(chunk) => bytes += chunk.len(),
                ProcessEvent::Exit { truncated, .. } => {
                    was_truncated = truncated;
                    break;
                }
            }
        }
        assert!(
            was_truncated,
            "stream emitted {bytes} bytes without truncation"
        );
        assert!(bytes <= 1024, "stream emitted {bytes} bytes");
    }
}
