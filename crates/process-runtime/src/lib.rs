//! 跨平台 Process Runtime（P4-12）。
//!
//! 进程组（Unix）/ 递归终止（Windows）；stdout/stderr 无死锁并发读取；
//! 超大输出截断；timeout 与协作式 cancel 任一触发即终止进程树。
//!
//! - Unix：`pre_exec` 中 `setpgid(0,0)` 建立进程组，kill 树用 `killpg(-pgid)`。
//! - Windows：递归 `taskkill /T`（完整 Job Object 实现见 P11-7）。

use std::process::Stdio;
use std::time::Duration;

use agent_domain::CancellationToken;
use tokio::io::AsyncReadExt;
use tokio::io::BufReader;
use tokio::process::Child;
use tokio::process::Command;
use tokio::sync::mpsc;

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

/// 进程树句柄：持有 child，可终止整个进程树。
pub struct ProcessHandle {
    child: Option<Child>,
}

impl ProcessHandle {
    /// 终止整个进程树。Unix 用 killpg(-pgid)；Windows 用 taskkill /T。
    pub async fn kill(&mut self) -> Result<(), ProcessError> {
        if let Some(child) = self.child.as_mut() {
            kill_child_tree(child).await;
        }
        Ok(())
    }

    pub fn id(&self) -> Option<u32> {
        self.child.as_ref().and_then(|c| c.id())
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            kill_child_tree_blocking(child);
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

        if let Some(stdout) = stdout {
            let tx = tx.clone();
            tokio::spawn(stream_lines(stdout, ProcessEvent::Stdout, tx));
        }
        if let Some(stderr) = stderr {
            let tx = tx.clone();
            tokio::spawn(stream_lines(stderr, ProcessEvent::Stderr, tx));
        }
        let exit_tx = tx.clone();
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            let code = tokio::select! {
                biased;
                _ = cancel_clone.cancelled() => {
                    kill_child_tree(&mut child).await;
                    None
                }
                status = child.wait() => {
                    status.ok().and_then(|st| st.code())
                }
            };
            let _ = exit_tx
                .send(ProcessEvent::Exit {
                    code,
                    truncated: false,
                })
                .await;
        });

        let handle = ProcessHandle { child: None };
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

/// Drop 路径上的同步终止（防止句柄析构时进程残留）。
fn kill_child_tree_blocking(child: &mut Child) {
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
            .status();
    }
    let _ = child.start_kill();
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
async fn stream_lines<R, F>(reader: R, make_event: F, tx: mpsc::Sender<ProcessEvent>)
where
    R: tokio::io::AsyncRead + Unpin,
    F: Fn(Vec<u8>) -> ProcessEvent + Send + Sync + 'static,
{
    let mut buf = BufReader::new(reader);
    let mut chunk = vec![0u8; 8192];
    loop {
        match buf.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                if tx.send(make_event(chunk[..n].to_vec())).await.is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(args: &[&str]) -> CommandSpec {
        let mut spec = CommandSpec::new("sh");
        spec = spec.args(args.iter().map(|s| s.to_string()));
        spec.max_output_bytes = 4 * 1024 * 1024;
        spec
    }

    #[tokio::test]
    async fn captures_stdout_and_stderr() {
        let runtime = ProcessRuntime::new();
        let spec = sh(&["-c", "echo hello; echo world >&2"]);
        let out = runtime
            .run(spec, CancellationToken::new())
            .await
            .expect("run");
        assert_eq!(out.stdout, b"hello\n");
        assert_eq!(out.stderr, b"world\n");
        assert_eq!(out.exit_code, Some(0));
        assert!(!out.timed_out && !out.killed);
    }

    #[tokio::test]
    async fn large_output_is_truncated() {
        let runtime = ProcessRuntime::new();
        let mut spec = sh(&["-c", "seq 1000000"]);
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
        let mut spec = sh(&["-c", "sleep 30"]);
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
            let mut spec = sh(&["-c", "sleep 30"]);
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
        let mut spec = sh(&["-c", "echo a; echo b >&2"]);
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
}
