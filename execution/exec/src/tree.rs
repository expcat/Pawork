//! 进程树生命周期守卫与整树终止。

use std::time::Duration;

use tokio::process::Child;

use crate::process::ProcessLimits;

pub(crate) const PROCESS_TREE_KILL_TIMEOUT: Duration = Duration::from_secs(5);

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
    job: crate::os::windows::Job,
}

impl ProcessTreeGuard {
    /// 绑定由其他进程启动器创建的子进程。
    ///
    /// Unix 要求目标已经是自己的 process-group leader（PTY 子进程经 `setsid` 满足）；
    /// Windows 为目标创建并绑定带 `KILL_ON_JOB_CLOSE` 的 Job Object。
    ///
    /// # `limits` 参数语义
    ///
    /// 仅 Windows 生效。Unix 分支忽略该参数（`let _ = limits`），只用 `pgid`
    /// 构造守卫；Windows 分支用 `limits` 创建 Job Object，并向
    /// `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` 写入限制。
    ///
    /// # 前置条件（Unix）
    ///
    /// 目标进程必须是自身进程组的 leader（`pgid == pid`），否则返回
    /// [`std::io::ErrorKind::InvalidInput`]。调用方须保证该前置条件——例如
    /// `pty-service` 经 portable-pty 的 `setsid` 满足。
    ///
    /// # 耗时（Windows 收养既有后代）
    ///
    /// 为收编绑定窗口内已产生的后代，本函数在 `spawn_blocking` 内同步执行
    /// `adopt_existing_descendants`：封顶 `MAX_ADOPTION_ROUNDS = 16` 轮，每轮
    /// 做一次全量进程快照与句柄操作，最坏耗时取决于系统进程数。仅
    /// `attach_external`（PTY 会话）路径触发收养；`spawn_stream` 自身以
    /// `CREATE_SUSPENDED` 启动子进程，不收养既有后代。
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
            let root_start_time = crate::os::linux::linux_process_tree::start_time(pid)?;
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
                job: crate::os::windows::Job::attach_pid(process_id, limits)?,
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (process_id, limits);
            Ok(Self {})
        }
    }

    pub(crate) fn attach(child: &Child, limits: ProcessLimits) -> std::io::Result<Self> {
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
            let root_start_time = crate::os::linux::linux_process_tree::start_time(pgid)?;
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
                job: crate::os::windows::Job::attach(child, limits)?,
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
                crate::os::linux::linux_process_tree::terminate(
                    self.root_pid,
                    self.pgid,
                    self.root_start_time,
                )
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
pub(crate) async fn kill_child_tree(child: &mut Child, tree: &ProcessTreeGuard) {
    let _ = tree.terminate();
    let _ = child.start_kill();
    let _ = tokio::time::timeout(PROCESS_TREE_KILL_TIMEOUT, child.wait()).await;
}
