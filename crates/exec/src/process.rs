//! 跨平台 Process Runtime。
//!
//! 进程组（Unix）/ 递归终止（Windows）；stdout/stderr 无死锁并发读取；
//! 超大输出截断；timeout 与协作式 cancel 任一触发即终止进程树。
//!
//! - Unix：`pre_exec` 中 `setpgid(0,0)` 建立进程组，kill 树用 `killpg(pgid)`；
//!   Linux / macOS 额外冻结并遍历后代（`/proc` 或 libproc），回收已通过
//!   `setsid` 离组的进程。
//! - Windows：Job Object 绑定整棵进程树，句柄关闭即回收后代。

use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::BufReader;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::process::{Child, ChildStdin};
use tokio::sync::{mpsc, watch, Mutex};

use crate::cancel::CancellationToken;
use crate::tree::{kill_child_tree, ProcessTreeGuard, PROCESS_TREE_KILL_TIMEOUT};

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

/// 受控子进程的 stdin 写入端。
///
/// 写入仍属于 [`ProcessRuntime`] 创建并监督的同一进程；clone 仅共享串行化写入端，
/// 不会创建额外进程或绕过进程树生命周期。
#[derive(Clone)]
pub struct ProcessInput {
    inner: Arc<Mutex<Option<ChildStdin>>>,
}

impl ProcessInput {
    fn new(stdin: ChildStdin) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Some(stdin))),
        }
    }

    /// 完整写入一段字节并 flush，避免协议帧在用户态缓冲区中滞留。
    pub async fn write_all(&self, bytes: &[u8]) -> Result<(), ProcessError> {
        let mut guard = self.inner.lock().await;
        let stdin = guard.as_mut().ok_or_else(|| {
            ProcessError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "process stdin is closed",
            ))
        })?;
        stdin.write_all(bytes).await?;
        stdin.flush().await?;
        Ok(())
    }

    /// 关闭 stdin；幂等。进程生命周期仍由 [`ProcessHandle`] 负责。
    pub async fn close(&self) -> Result<(), ProcessError> {
        let mut guard = self.inner.lock().await;
        if let Some(mut stdin) = guard.take() {
            stdin.shutdown().await?;
        }
        Ok(())
    }
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
        let (mut child, tree) = spawn_child(&spec, false)?;
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
        let (events, input, handle) = self.spawn_stream_inner(spec, cancel, false).await?;
        debug_assert!(input.is_none());
        Ok((events, handle))
    }

    /// 双向流式执行：除 stdout/stderr 事件外返回受控 stdin 写入端。
    ///
    /// LSP、MCP 等 Core-owned 长驻协议进程必须使用本入口，再由 Sandbox Runtime
    /// 包装；它与 [`Self::spawn_stream`] 共用 timeout/cancel/进程树监督状态机。
    pub async fn spawn_interactive(
        &self,
        spec: CommandSpec,
        cancel: CancellationToken,
    ) -> Result<(mpsc::Receiver<ProcessEvent>, ProcessInput, ProcessHandle), ProcessError> {
        let (events, input, handle) = self.spawn_stream_inner(spec, cancel, true).await?;
        let input = input.ok_or_else(|| {
            ProcessError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "interactive process did not expose stdin",
            ))
        })?;
        Ok((events, input, handle))
    }

    async fn spawn_stream_inner(
        &self,
        spec: CommandSpec,
        cancel: CancellationToken,
        pipe_stdin: bool,
    ) -> Result<
        (
            mpsc::Receiver<ProcessEvent>,
            Option<ProcessInput>,
            ProcessHandle,
        ),
        ProcessError,
    > {
        let (mut child, tree) = spawn_child(&spec, pipe_stdin)?;
        let input = child.stdin.take().map(ProcessInput::new);
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
        Ok((rx, input, handle))
    }
}

/// spawn 子进程并立即绑定平台进程树守卫。
fn spawn_child(
    spec: &CommandSpec,
    pipe_stdin: bool,
) -> Result<(Child, ProcessTreeGuard), ProcessError> {
    let mut command = Command::new(&spec.program);
    command.args(&spec.args);
    command.stdin(if pipe_stdin {
        Stdio::piped()
    } else {
        Stdio::null()
    });
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
            if let Err(source) = crate::os::windows::resume(&child) {
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
        .map(crate::os::linux::prepare_linux_landlock)
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
            // Darwin 对有限 RLIMIT_AS 返回 EINVAL（当前地址空间上限为 RLIM_INFINITY
            // 且不可降）。软内存上限尽力而为，不能因此让整个 spawn 失败。
            let value = memory_bytes.min(libc::rlim_t::MAX as u64) as libc::rlim_t;
            let limit = libc::rlimit {
                rlim_cur: value,
                rlim_max: value,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &limit) == -1 {
                let err = std::io::Error::last_os_error();
                let skip_einval =
                    cfg!(target_os = "macos") && err.raw_os_error() == Some(libc::EINVAL);
                if !skip_einval {
                    return Err(err);
                }
            }
        }
        if let Some(open_files) = limits.open_files {
            set_limit!(libc::RLIMIT_NOFILE, open_files);
        }
        // Darwin（以及多数 Unix）的 RLIMIT_NPROC 按 uid 计数，不是按进程树。
        // 开发机上用户进程数常远大于 64，setrlimit 成功后下一次 fork 直接 EAGAIN
        //（`sh: fork: Resource temporarily unavailable`）。macOS 跳过；Linux 交给
        // bwrap PID namespace / 后续 cgroup，不在这里误伤。
        #[cfg(not(target_os = "macos"))]
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

                crate::os::linux::restrict_linux_landlock(ruleset_fd.as_raw_fd())?;
            }
        }
        Ok(())
    });
    Ok(())
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
    async fn memory_limit_does_not_fail_spawn() {
        let runtime = ProcessRuntime::new();
        #[cfg(not(windows))]
        let mut spec = shell("echo ok");
        #[cfg(windows)]
        let mut spec = shell("echo ok");
        spec.limits.memory_bytes = Some(2 * 1024 * 1024 * 1024);
        let out = runtime
            .run(spec, CancellationToken::new())
            .await
            .expect("spawn with memory limit");
        assert_eq!(out.exit_code, Some(0));
        #[cfg(not(windows))]
        assert_eq!(out.stdout, b"ok\n");
        #[cfg(windows)]
        assert_eq!(out.stdout, b"ok\r\n");
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

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn escaped_setsid_script() -> Option<String> {
        fn available(program: &str, args: &[&str]) -> bool {
            std::process::Command::new(program)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|status| status.success())
                .unwrap_or(false)
        }
        if available("setsid", &["--version"]) {
            return Some(
                "setsid sh -c 'echo $$; sleep 30' & escaped=$!; wait $escaped".to_string(),
            );
        }
        if available("perl", &["-MPOSIX", "-e", "setsid()"]) {
            return Some(
                r#"perl -MPOSIX -e 'setsid(); $| = 1; print "$$\n"; exec "sleep", "30" or die $!' & escaped=$!; wait $escaped"#
                    .to_string(),
            );
        }
        None
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn kill_reaps_descendant_that_escaped_with_setsid() {
        let script = match escaped_setsid_script() {
            Some(script) => script,
            None => {
                #[cfg(target_os = "macos")]
                panic!("macOS must reap setsid descendants; perl POSIX::setsid was unavailable");
                #[cfg(not(target_os = "macos"))]
                return;
            }
        };

        let runtime = ProcessRuntime::new();
        let spec = shell(&script);
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
