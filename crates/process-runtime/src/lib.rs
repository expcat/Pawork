//! 跨平台 Process Runtime（P4-12）。
//!
//! 进程组（Unix）/ 递归终止（Windows）；stdout/stderr 无死锁并发读取；
//! 超大输出截断；timeout 与协作式 cancel 任一触发即终止进程树。
//!
//! - Unix：`pre_exec` 中 `setpgid(0,0)` 建立进程组，kill 树用 `killpg(-pgid)`。
//! - Windows：递归 `taskkill /T`（完整 Job Object 实现见 P11-7）。

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
    /// 终止整个进程树。Unix 用 killpg(-pgid)；Windows 用 taskkill /T。
    pub async fn kill(&mut self) -> Result<(), ProcessError> {
        self.kill.cancel();
        while !*self.done.borrow() {
            if self.done.changed().await.is_err() {
                break;
            }
        }
        Ok(())
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
        let mut child = spawn_child(&spec)?;
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
                    kill_child_tree(&mut child).await;
                    killed = true;
                    None
                }
                _ = tokio::time::sleep(dur) => {
                    kill_child_tree(&mut child).await;
                    timed_out = true;
                    None
                }
                status = child.wait() => {
                    status.ok().and_then(|st| st.code())
                }
            }
        } else {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    kill_child_tree(&mut child).await;
                    killed = true;
                    None
                }
                status = child.wait() => {
                    status.ok().and_then(|st| st.code())
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
        let mut child = spawn_child(&spec)?;
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
                        kill_child_tree(&mut child).await;
                        None
                    }
                    _ = kill_for_task.cancelled() => {
                        kill_child_tree(&mut child).await;
                        None
                    }
                    _ = tokio::time::sleep(duration) => {
                        kill_child_tree(&mut child).await;
                        None
                    }
                    status = child.wait() => {
                        status.ok().and_then(|st| st.code())
                    }
                }
            } else {
                tokio::select! {
                    biased;
                    _ = cancel_clone.cancelled() => {
                        kill_child_tree(&mut child).await;
                        None
                    }
                    _ = kill_for_task.cancelled() => {
                        kill_child_tree(&mut child).await;
                        None
                    }
                    status = child.wait() => {
                        status.ok().and_then(|st| st.code())
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

/// spawn 子进程，返回裸 [`Child`]。
fn spawn_child(spec: &CommandSpec) -> Result<Child, ProcessError> {
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
    #[cfg(unix)]
    {
        // SAFETY: 仅调用 setpgid 建立进程组，不触及不安全内存。
        unsafe { setpgid_pre_exec(&mut command) };
    }
    command.spawn().map_err(|source| ProcessError::Spawn {
        program: spec.program.clone(),
        source,
    })
}

#[cfg(unix)]
unsafe fn setpgid_pre_exec(command: &mut Command) {
    command.pre_exec(|| {
        libc::setpgid(0, 0);
        Ok(())
    });
}

/// 终止整个进程树（Unix 进程组 / Windows taskkill），并尽量回收子进程。
async fn kill_child_tree(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let pgid = pid as i32;
        unsafe {
            libc::killpg(-pgid, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
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
    async fn stream_output_respects_shared_limit() {
        let runtime = ProcessRuntime::new();
        #[cfg(not(windows))]
        let mut spec = shell("yes x | head -c 8192; yes y | head -c 8192 >&2");
        #[cfg(windows)]
        let mut spec = shell(
            "powershell -NoProfile -Command \"'x' * 8192; [Console]::Error.Write('y' * 8192)\"",
        );
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
        assert!(was_truncated);
        assert!(bytes <= 1024, "stream emitted {bytes} bytes");
    }
}
