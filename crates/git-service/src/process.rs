//! 系统 git 的统一调用入口 [`GitRunner`]。
//!
//! 所有 git 命令经 `process_runtime::ProcessRuntime` 执行，统一设置 `cwd`、
//! `timeout`、`max_output_bytes`，并把 `ProcessOutput` 归一为 [`GitError`]。
//! stdout/stderr 以 lossy UTF-8 返回，非零退出视为错误。

use std::path::Path;
use std::time::Duration;

use agent_domain::CancellationToken;
use process_runtime::{CommandSpec, ProcessRuntime};

use crate::error::GitError;

/// 默认调用超时（30s）。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// 单次输出上限（16MB），防止巨量 diff/log 打爆内存。
const MAX_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;

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
}

impl GitRunner {
    /// 默认构造：`git` 路径、30s 超时、新建 [`ProcessRuntime`]。
    pub fn new() -> Self {
        Self {
            runtime: ProcessRuntime::new(),
            git_path: "git".to_string(),
            default_timeout: DEFAULT_TIMEOUT,
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
        }
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
        let mut spec = CommandSpec::new(self.git_path.as_str()).args(args.iter().copied());
        spec.cwd = Some(cwd.to_path_buf());
        spec.timeout = Some(self.default_timeout);
        spec.max_output_bytes = MAX_OUTPUT_BYTES;
        // env_clear 保持 false：保留环境，便于 git 读取用户配置与 credential。

        let output = self.runtime.run(spec, cancel).await?;
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

impl Default for GitRunner {
    fn default() -> Self {
        Self::new()
    }
}
