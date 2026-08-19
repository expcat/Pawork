//! 系统 git 的统一调用入口 [`GitRunner`]。
//!
//! 所有 git 命令经 `pawork_exec::ProcessRuntime` 执行，统一设置 `cwd`、
//! `timeout`、`max_output_bytes`，并把 `ProcessOutput` 归一为 [`GitError`]。
//! stdout/stderr 以 lossy UTF-8 返回，非零退出视为错误。

use std::path::Path;
use std::time::Duration;
#[cfg(test)]
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    sync::Arc,
};

use pawork_domain::CancellationToken;
use pawork_exec::CancellationToken as ExecCancellationToken;
use pawork_exec::{CommandSpec, ProcessRuntime};

use crate::error::GitError;

/// 把 domain 取消令牌桥到 exec 令牌：已取消则立刻 cancel；否则后台等待后再 cancel。
///
/// 与 `pawork-tools` `run_command` 的桥接同形：两种 `CancellationToken` 不可互转。
fn bridge_exec_cancel(
    domain: &CancellationToken,
) -> (ExecCancellationToken, Option<tokio::task::JoinHandle<()>>) {
    let exec = ExecCancellationToken::new();
    if domain.is_cancelled() {
        exec.cancel();
        return (exec, None);
    }
    let domain = domain.clone();
    let exec_for_wait = exec.clone();
    let handle = tokio::spawn(async move {
        domain.cancelled().await;
        exec_for_wait.cancel();
    });
    (exec, Some(handle))
}

/// 默认调用超时（30s）。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// 单次输出上限（16MB），防止巨量 diff/log 打爆内存。
const MAX_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;

/// 校验会被 git 当作位置参数解析的 revision / range / branch。
///
/// 这些值来自上层（包括模型输出），若以 `-` 开头会被 git 重新解释为选项。
/// 路径参数应使用 `--` 分隔，不走本校验。
pub fn validate_position_arg(name: &'static str, value: &str) -> Result<(), GitError> {
    if value.starts_with('-') {
        return Err(GitError::InvalidPositionArgument {
            name,
            value: value.to_string(),
        });
    }
    Ok(())
}

/// 系统 git 的统一调用入口。
///
/// 持有 [`ProcessRuntime`]、git 可执行路径与默认超时；[`GitRunner::run`] 与
/// [`GitRunner::run_with_stderr`] 返回 lossy UTF-8 文本，把非零退出 / 超时 / 取消
/// 归一为 [`GitError`]。
#[derive(Clone, Debug)]
pub struct GitRunner {
    runtime: ProcessRuntime,
    git_path: String,
    default_timeout: Duration,
    #[cfg(test)]
    call_count: Option<Arc<AtomicUsize>>,
}

impl GitRunner {
    /// 默认构造：`git` 路径、30s 超时、新建 [`ProcessRuntime`]。
    pub fn new() -> Self {
        Self {
            runtime: ProcessRuntime::new(),
            git_path: "git".to_string(),
            default_timeout: DEFAULT_TIMEOUT,
            #[cfg(test)]
            call_count: None,
        }
    }

    /// 用指定 runtime / git 路径 / 超时构造，便于测试注入。
    pub fn with_runtime(
        runtime: ProcessRuntime,
        git_path: impl Into<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            runtime,
            git_path: git_path.into(),
            default_timeout: timeout,
            #[cfg(test)]
            call_count: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_call_count(mut self, call_count: Arc<AtomicUsize>) -> Self {
        self.call_count = Some(call_count);
        self
    }

    /// 在 `cwd` 下执行 `git <args>`，stdout 以 lossy UTF-8 返回。
    ///
    /// 非零退出 → [`GitError::GitFailed`]；超时 → [`GitError::Timeout`]；
    /// cancel → [`GitError::Cancelled`]。
    pub async fn run(
        &self,
        cwd: &Path,
        args: &[&str],
        cancel: CancellationToken,
    ) -> Result<String, GitError> {
        let (stdout, _stderr) = self.run_with_stderr(cwd, args, cancel).await?;
        Ok(stdout)
    }

    /// 同 [`GitRunner::run`]，但额外返回 stderr（仍以非零退出为错误）。
    pub async fn run_with_stderr(
        &self,
        cwd: &Path,
        args: &[&str],
        cancel: CancellationToken,
    ) -> Result<(String, String), GitError> {
        #[cfg(test)]
        if let Some(call_count) = &self.call_count {
            call_count.fetch_add(1, Ordering::Relaxed);
        }
        let mut spec = CommandSpec::new(self.git_path.as_str()).args(args.iter().copied());
        // Windows canonicalize 可能产生 `\\?\` verbatim 前缀；部分 git 版本不能
        // 稳定处理该形式，因此在唯一子进程出口统一简化。
        spec.cwd = Some(simplified_cwd(cwd));
        spec.timeout = Some(self.default_timeout);
        spec.max_output_bytes = MAX_OUTPUT_BYTES;
        // env_clear 保持 false：保留环境，便于 git 读取用户配置与 credential。

        let (exec_cancel, cancel_bridge) = bridge_exec_cancel(&cancel);
        let result = self.runtime.run(spec, exec_cancel).await;
        if let Some(handle) = cancel_bridge {
            handle.abort();
        }
        let output = result?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

        if output.timed_out {
            return Err(GitError::Timeout);
        }
        if output.killed && output.exit_code.is_none() {
            return Err(GitError::Cancelled);
        }
        match output.exit_code {
            Some(0) => Ok((stdout, stderr)),
            code => Err(GitError::GitFailed { code, stderr }),
        }
    }
}

fn simplified_cwd(cwd: &Path) -> std::path::PathBuf {
    dunce::simplified(cwd).to_path_buf()
}

impl Default for GitRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_option_like_position_arguments() {
        let error = validate_position_arg("revision", "--help").expect_err("must reject");
        assert!(matches!(
            error,
            GitError::InvalidPositionArgument {
                name: "revision",
                ref value
            } if value == "--help"
        ));
        validate_position_arg("revision", "HEAD~1..HEAD").expect("valid revision range");
    }

    #[test]
    fn relative_cwd_is_unchanged() {
        assert_eq!(
            simplified_cwd(Path::new("repo/subdir")),
            Path::new("repo/subdir")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_verbatim_cwd_is_simplified() {
        let simplified = simplified_cwd(Path::new(r"\\?\C:\repo\worktree"));
        assert_eq!(simplified, Path::new(r"C:\repo\worktree"));
        assert!(!simplified.to_string_lossy().starts_with(r"\\?\"));
    }
}
