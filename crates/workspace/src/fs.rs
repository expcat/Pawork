//! 同目录临时文件 + fsync + rename。
//!
//! 配置写盘与 compat 导出共用。临时名带 pid、进程级计数与纳秒，且不以
//! `.tmp-` 开头（该前缀属于 storage artifact GC）。不检查目标路径本身是否为 symlink。

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 原子替换 `path` 的内容。
///
/// 需要时创建父目录；在同目录以 `create_new` 打开临时文件，`write_all` +
/// `sync_all` 后 `rename` 覆盖目标。任一步失败都删除本次创建的临时文件。
pub(crate) fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = stage_exclusive_temp(path, bytes)?;
    commit_exclusive_temp(&temp, path)
}

/// 写入同目录独占临时文件并 fsync，返回临时路径。
///
/// 调用方可以在 rename 前插入检查；放弃或失败时删除该临时文件。
pub(crate) fn stage_exclusive_temp(target: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let temp_path = unique_temp_path(target);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)?;
    let staged = (|| -> io::Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })();
    drop(file);
    if let Err(error) = staged {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    Ok(temp_path)
}

pub(crate) fn discard_exclusive_temp(temp: &Path) {
    let _ = fs::remove_file(temp);
}

pub(crate) fn commit_exclusive_temp(temp: &Path, target: &Path) -> io::Result<()> {
    match fs::rename(temp, target) {
        Ok(()) => Ok(()),
        Err(error) => {
            discard_exclusive_temp(temp);
            Err(error)
        }
    }
}

fn unique_temp_path(target: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let stem = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "pawork".to_string());
    target.with_file_name(format!(".{stem}.{}.{n}.{nanos}.tmp", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("read dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn writes_overwrites_and_creates_parent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("out.txt");
        atomic_write_bytes(&path, b"one").expect("create");
        assert_eq!(fs::read(&path).expect("read"), b"one");
        atomic_write_bytes(&path, b"two").expect("overwrite");
        assert_eq!(fs::read(&path).expect("read"), b"two");
        assert_eq!(
            names(&dir.path().join("nested")),
            vec!["out.txt".to_string()]
        );
    }

    #[test]
    fn rename_failure_leaves_no_temp_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("occupied");
        fs::create_dir(&target).expect("target dir");
        atomic_write_bytes(&target, b"payload").expect_err("rename onto directory");
        assert_eq!(names(dir.path()), vec!["occupied".to_string()]);
    }
}
