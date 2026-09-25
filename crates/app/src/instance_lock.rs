//! 实例级 Host 所有权（R-04 / R-24）。
//!
//! 目标：
//! - **R-24**：普通查询（catalog 装配）不得触发启动清扫；只有确认无其他
//!   活跃宿主的执行型宿主才能清扫崩溃遗留的 running run。
//! - **R-04**：同一实例只允许一个 GUI Host；所有权获取先于打开库与
//!   bind socket，替代「先探测再绑定」的竞态。
//!
//! 机制（全部落在实例目录内，复用 pawork-auth 的文件锁原语——锁由内核
//! 随进程退出释放，不存在 PID 复用问题）：
//! - `gui.lock`：GUI Host 单实例锁，GuiHost 角色独占持有整个生命周期；
//!   获取失败即拒绝装配。
//! - `sweep.lock`：登记/清扫互斥锁，清扫权判定与活跃登记在其临界区内
//!   完成，消除「枚举之后再登记」的窗口。
//! - `hosts/<pid>-<纳秒>-<序号>.lock`：每个执行型进程的活跃登记，进程
//!   持有独占锁直至 core 丢弃；清扫方逐一试锁——锁不到即活跃宿主，
//!   锁得到即属主已死（stale），可安全清理并清扫。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use pawork_auth::{acquire_file_lock, try_acquire_file_lock, FileLockGuard};

use crate::AppError;

/// 装配角色：决定实例锁获取与启动清扫（seal_interrupted_runs）资格。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InstanceRole {
    /// 只读 / 目录查询命令：不取任何锁，永不参与启动清扫（R-24）。
    #[default]
    Catalog,
    /// 执行型宿主（chat / run / headless / acp / agents demo）：登记活跃
    /// 记录；仅当确认无其他活跃宿主时才清扫。
    Executor,
    /// GUI 宿主：在 Executor 之上额外独占 `gui.lock`（单实例，R-04）。
    GuiHost,
}

/// 清扫互斥的有界等待：正常清扫亚秒级；异常长清扫宁可报错也不无限等。
const SWEEP_MUTEX_TIMEOUT: Duration = Duration::from_secs(30);
const GUI_LOCK_FILE: &str = "gui.lock";
const SWEEP_LOCK_FILE: &str = "sweep.lock";
const HOSTS_DIR: &str = "hosts";

/// 活跃属主诊断：只报告仍持登记锁的进程，不清扫或改写任务快照。
pub fn active_instance_host_pids(instance_dir: &Path) -> Result<Vec<u32>, AppError> {
    let entries = match std::fs::read_dir(instance_dir.join(HOSTS_DIR)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut pids = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|name| name.split('-').next())
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        if try_acquire_file_lock(&entry.path())?.is_none() {
            pids.push(pid);
        }
    }
    pids.sort_unstable();
    pids.dedup();
    Ok(pids)
}

/// 进程级计数器：同进程同纳秒重复登记也不会撞名（见 AGENTS.md 工程经验）。
static REGISTRATION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 实例所有权持有体：放入 `AppCore`，随 core 丢弃释放全部锁。
pub(crate) struct InstanceOwnership {
    /// 本进程活跃登记；None 仅当 Catalog 角色。靠持锁与 Drop 起效。
    #[allow(dead_code)]
    registration: Option<Registration>,
    /// 清扫互斥：获得清扫权时持有到 `finish_sweep`；否则登记后立即释放。
    sweep_mutex: Option<FileLockGuard>,
    /// GUI 单实例锁（仅 GuiHost）；与活跃登记一起持有至 Core 退出。
    _gui_lock: Option<FileLockGuard>,
    can_sweep: bool,
}

struct Registration {
    _guard: FileLockGuard,
    path: PathBuf,
}

impl Drop for Registration {
    fn drop(&mut self) {
        // 干净退出主动删除登记文件；进程被杀时文件遗留但锁已释放，
        // 下一个获得清扫权的宿主会按 stale 清理。
        let _ = std::fs::remove_file(&self.path);
    }
}

impl InstanceOwnership {
    pub(crate) fn can_sweep(&self) -> bool {
        self.can_sweep
    }

    /// 启动清扫完成后释放清扫互斥（活跃登记继续持有）。
    pub(crate) fn finish_sweep(&mut self) {
        self.sweep_mutex = None;
    }
}

/// 按角色获取实例所有权。Catalog 不触碰任何文件。
///
/// GuiHost 获取 `gui.lock` 失败时报 [`AppError::InstanceLocked`]——
/// 该错误发生在打开库与 bind socket 之前，是唯一的单实例判定（R-04）。
pub(crate) fn acquire_instance_ownership(
    instance_dir: &Path,
    role: InstanceRole,
) -> Result<InstanceOwnership, AppError> {
    if role == InstanceRole::Catalog {
        return Ok(InstanceOwnership {
            registration: None,
            sweep_mutex: None,
            _gui_lock: None,
            can_sweep: false,
        });
    }
    std::fs::create_dir_all(instance_dir)?;
    let gui_lock = if role == InstanceRole::GuiHost {
        let path = instance_dir.join(GUI_LOCK_FILE);
        match try_acquire_file_lock(&path)? {
            Some(guard) => Some(guard),
            None => {
                return Err(AppError::InstanceLocked(format!(
                    "{}（{} 已被持有）",
                    instance_dir.display(),
                    path.display()
                )));
            }
        }
    } else {
        None
    };

    let hosts_dir = instance_dir.join(HOSTS_DIR);
    std::fs::create_dir_all(&hosts_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&hosts_dir, std::fs::Permissions::from_mode(0o700));
    }

    // 登记与清扫判定互斥：持锁期间枚举 + 试锁全部登记，消除
    // 「枚举为空 → 他人登记 → 我清扫」的交错窗口。
    let sweep_mutex = acquire_file_lock(&instance_dir.join(SWEEP_LOCK_FILE), SWEEP_MUTEX_TIMEOUT)?;
    let mut can_sweep = true;
    for entry in std::fs::read_dir(&hosts_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        match try_acquire_file_lock(&path)? {
            // 试锁成功 = 属主进程已退出（锁随进程释放）：清理 stale 登记。
            Some(stale) => {
                drop(stale);
                let _ = std::fs::remove_file(&path);
            }
            // 锁不到 = 有活跃宿主：本次不清扫。
            None => can_sweep = false,
        }
    }
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let registration_path = hosts_dir.join(format!(
        "{}-{}-{}.lock",
        std::process::id(),
        nanos,
        REGISTRATION_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    // 全新文件名，加锁必然成功；仍走超时获取以复用同一原语。
    let registration_guard = acquire_file_lock(&registration_path, SWEEP_MUTEX_TIMEOUT)?;
    let registration = Registration {
        _guard: registration_guard,
        path: registration_path,
    };
    Ok(InstanceOwnership {
        registration: Some(registration),
        // 不清扫时立即释放互斥；获得清扫权则持到 open_control_plane 完成 Task 恢复。
        sweep_mutex: if can_sweep { Some(sweep_mutex) } else { None },
        _gui_lock: gui_lock,
        can_sweep,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_role_creates_no_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ownership =
            acquire_instance_ownership(dir.path(), InstanceRole::Catalog).expect("catalog");
        assert!(!ownership.can_sweep());
        assert!(std::fs::read_dir(dir.path())
            .expect("read dir")
            .next()
            .is_none());
    }

    #[test]
    fn first_executor_sweeps_second_waits_for_ownership() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut first =
            acquire_instance_ownership(dir.path(), InstanceRole::Executor).expect("first");
        assert!(first.can_sweep());
        // 生产路径中 Run 与 Task 恢复完成后才调用 finish_sweep 释放互斥。
        first.finish_sweep();
        let second =
            acquire_instance_ownership(dir.path(), InstanceRole::Executor).expect("second");
        assert!(!second.can_sweep(), "活跃宿主存在时旁观者不得获得清扫权");
        // 第一名仍持登记时，其登记文件真实存在。
        assert!(
            dir.path()
                .join(HOSTS_DIR)
                .read_dir()
                .expect("hosts")
                .count()
                >= 1
        );
    }

    #[test]
    fn sweep_right_returns_after_owner_drops() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = acquire_instance_ownership(dir.path(), InstanceRole::Executor).expect("first");
        assert!(first.can_sweep());
        drop(first);
        // 属主退出（锁随文件关闭释放）后，新宿主重新获得清扫权。
        let third = acquire_instance_ownership(dir.path(), InstanceRole::Executor).expect("third");
        assert!(third.can_sweep());
    }

    #[test]
    fn gui_host_conflict_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first =
            acquire_instance_ownership(dir.path(), InstanceRole::GuiHost).expect("first gui");
        assert!(first.can_sweep());
        let result = acquire_instance_ownership(dir.path(), InstanceRole::GuiHost);
        assert!(
            matches!(result, Err(AppError::InstanceLocked(_))),
            "second gui host must be rejected with InstanceLocked"
        );
    }

    #[test]
    fn executor_coexists_with_gui_host_without_sweeping() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut gui = acquire_instance_ownership(dir.path(), InstanceRole::GuiHost).expect("gui");
        gui.finish_sweep();
        let executor =
            acquire_instance_ownership(dir.path(), InstanceRole::Executor).expect("executor");
        assert!(!executor.can_sweep(), "GUI 宿主活跃时执行型宿主不得清扫");
        drop(gui);
    }

    #[test]
    fn stale_registration_is_cleaned_by_next_owner() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hosts_dir = dir.path().join(HOSTS_DIR);
        std::fs::create_dir_all(&hosts_dir).expect("hosts dir");
        // 伪造崩溃遗留：加锁后立即释放锁但留下文件。
        let stale_path = hosts_dir.join("999999-0-0.lock");
        {
            let stale = acquire_file_lock(&stale_path, Duration::from_secs(1)).expect("stale");
            drop(stale);
        }
        assert!(stale_path.exists());
        let owner = acquire_instance_ownership(dir.path(), InstanceRole::Executor).expect("owner");
        assert!(owner.can_sweep(), "无活跃宿主时必须获得清扫权");
        assert!(!stale_path.exists(), "stale 登记文件应被清理");
    }

    #[test]
    fn sweep_mutex_is_released_by_finish_sweep() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut first =
            acquire_instance_ownership(dir.path(), InstanceRole::Executor).expect("first");
        first.finish_sweep();
        // 互斥已释放：第二个获取方立即返回（否则测试会卡满 30s 超时）。
        let second =
            acquire_instance_ownership(dir.path(), InstanceRole::Executor).expect("second");
        assert!(!second.can_sweep());
    }
}
