//! 同目录临时文件 + fsync + rename，供 artifact / protected / checkpoint 共用。
//!
//! 临时名一律 `.tmp-` 前缀，以便 artifact GC（跳过写入中文件、超过 24h 回收崩溃残留）
//! 继续识别。

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 原子替换 `path` 的内容。
///
/// 确保父目录存在，在同目录写入唯一 `.tmp-{pid}-{counter}`（`create_new`），
/// `write_all` + `sync_all` 后 `rename` 覆盖目标；任一步失败则尽力删除临时文件。
pub(crate) fn atomic_write_bytes(path: &Path, content: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let temp_path = path.with_file_name(format!(
        ".tmp-{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> io::Result<()> {
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)?;
            file.write_all(content)?;
            file.sync_all()?;
        }
        fs::rename(&temp_path, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}
