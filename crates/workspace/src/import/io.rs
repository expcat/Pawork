//! 有界、symlink 安全的只读文件访问与稳定哈希。
//!
//! 所有读取逐级 no-follow 解析：从（已 canonicalize 的）根出发，沿相对路径的
//! 每个 Normal 组件下钻，任一组件（含最终文件）为 symlink 即拒绝；最终用打开
//! 的句柄读取并按字节上限硬截断，避免 metadata 与 read 之间的 TOCTOU，并拒绝
//! FIFO / socket / 设备等特殊文件。读取结果用于内容指纹，保证幂等 apply 的
//! 输入一致性。

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use super::error::CompatError;

/// 相对路径转正斜杠字符串。
pub(crate) fn rel_key(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// 判断路径是否指向根内的常规文件（逐级 no-follow，拒绝任何 symlink 组件）。
pub(crate) fn is_file_within(root: &Path, rel: &Path) -> bool {
    open_file_no_follow(root, rel).is_ok()
}

/// 排序后的子目录项（不跟随 symlink 目录），按 max_entries 硬截断。
/// 返回 (条目, 是否因超限截断)；截断由调用方转为诊断。
pub(crate) fn sorted_children(
    dir: &Path,
    max_entries: usize,
) -> Result<(Vec<PathBuf>, bool), CompatError> {
    let mut entries: Vec<PathBuf> = Vec::new();
    let read = std::fs::read_dir(dir).map_err(|error| CompatError::io(dir, error))?;
    let mut truncated = false;
    for entry in read {
        if entries.len() >= max_entries {
            truncated = true;
            break;
        }
        let entry = entry.map_err(|error| CompatError::io(dir, error))?;
        entries.push(entry.path());
    }
    entries.sort();
    Ok((entries, truncated))
}

/// FNV-1a 64 位稳定哈希（仅用于幂等指纹，非安全用途）。
pub(crate) fn fnv64(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// 校验相对目标路径安全：全部为 Normal 组件、非空、非绝对路径。
pub(crate) fn validate_relative_target(rel: &str) -> Result<(), CompatError> {
    if rel.is_empty() {
        return Err(CompatError::UnsafeTarget(rel.to_string()));
    }
    let path = Path::new(rel);
    if path.is_absolute() {
        return Err(CompatError::UnsafeTarget(rel.to_string()));
    }
    for component in path.components() {
        match component {
            std::path::Component::Normal(_) => {}
            _ => return Err(CompatError::UnsafeTarget(rel.to_string())),
        }
    }
    Ok(())
}

/// 安全读取：逐级 no-follow 解析（拒绝任何 symlink 组件），用打开的句柄读取
/// 并按 max_bytes 硬截断；拒绝目录与 FIFO / socket / 设备等特殊文件，非
/// UTF-8 / 超限均作为错误返回。
pub(crate) fn read_utf8_bounded(
    root: &Path,
    rel: &Path,
    max_bytes: u64,
) -> Result<String, CompatError> {
    validate_relative_target(&rel_key(rel))?;
    let mut file = open_file_no_follow(root, rel)?;
    let metadata = file
        .metadata()
        .map_err(|error| CompatError::io(rel, error))?;
    if !metadata.is_file() {
        return Err(CompatError::Invalid(format!(
            "not a regular file: {}",
            rel_key(rel)
        )));
    }
    // 按句柄硬限：最多读 max_bytes + 1 字节，超出即拒绝（防 metadata 之后文件膨胀）。
    let cap = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    let mut buffer = Vec::with_capacity(cap.min(1 << 16));
    let mut limited = (&mut file).take(max_bytes.saturating_add(1));
    limited
        .read_to_end(&mut buffer)
        .map_err(|error| CompatError::io(rel, error))?;
    if buffer.len() > cap {
        return Err(CompatError::LimitExceeded(format!(
            "file exceeds {max_bytes} bytes: {}",
            rel_key(rel)
        )));
    }
    String::from_utf8(buffer)
        .map_err(|_| CompatError::Invalid(format!("not valid UTF-8: {}", rel_key(rel))))
}

/// 从 root 目录句柄出发逐级 no-follow 打开最终文件。Unix 使用 openat 句柄链，
/// Windows 每一级以 OPEN_REPARSE_POINT 打开并拒绝 reparse point；检查和读取均
/// 作用于最终打开的同一文件句柄。
fn open_file_no_follow(root: &Path, rel: &Path) -> Result<std::fs::File, CompatError> {
    validate_relative_target(&rel_key(rel))?;
    platform_no_follow::open_file(root, rel)
}

#[cfg(unix)]
mod platform_no_follow {
    use std::ffi::{CString, OsStr};
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    use super::{rel_key, CompatError, Component};

    pub(super) fn open_file(root: &Path, rel: &Path) -> Result<File, CompatError> {
        let root_name = c_string(root.as_os_str())?;
        let root_fd = unsafe {
            libc::open(
                root_name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if root_fd < 0 {
            return Err(CompatError::io(root, std::io::Error::last_os_error()));
        }
        let mut directory = unsafe { OwnedFd::from_raw_fd(root_fd) };
        let mut components = rel.components().peekable();
        while let Some(component) = components.next() {
            let Component::Normal(name) = component else {
                return Err(CompatError::OutOfBounds(rel_key(rel)));
            };
            let name = c_string(name)?;
            if components.peek().is_some() {
                let fd = unsafe {
                    libc::openat(
                        directory.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                };
                if fd < 0 {
                    return Err(no_follow_error(rel));
                }
                directory = unsafe { OwnedFd::from_raw_fd(fd) };
            } else {
                let fd = unsafe {
                    libc::openat(
                        directory.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                    )
                };
                if fd < 0 {
                    return Err(no_follow_error(rel));
                }
                let file = unsafe { File::from_raw_fd(fd) };
                if !file
                    .metadata()
                    .map_err(|error| CompatError::io(rel, error))?
                    .is_file()
                {
                    return Err(CompatError::Invalid(format!(
                        "not a regular file: {}",
                        rel_key(rel)
                    )));
                }
                return Ok(file);
            }
        }
        Err(CompatError::OutOfBounds(rel_key(rel)))
    }

    fn c_string(value: &OsStr) -> Result<CString, CompatError> {
        CString::new(value.as_bytes())
            .map_err(|_| CompatError::Invalid("path contains a NUL byte".to_string()))
    }

    fn no_follow_error(rel: &Path) -> CompatError {
        let error = std::io::Error::last_os_error();
        if matches!(
            error.raw_os_error(),
            Some(libc::ELOOP) | Some(libc::ENOTDIR)
        ) {
            CompatError::Invalid(format!("symlink component rejected: {}", rel_key(rel)))
        } else {
            CompatError::io(rel, error)
        }
    }
}

#[cfg(windows)]
mod platform_no_follow {
    use std::ffi::OsStr;
    use std::fs::{File, OpenOptions};
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::{Path, PathBuf};

    use super::{rel_key, CompatError, Component};

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    const FILE_ATTRIBUTE_DEVICE: u32 = 0x40;
    const FILE_ATTRIBUTE_TAG_INFO: u32 = 9;

    #[repr(C)]
    struct FileAttributeTagInfo {
        file_attributes: u32,
        reparse_tag: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetFileInformationByHandleEx(
            file: *mut std::ffi::c_void,
            info_class: u32,
            info: *mut std::ffi::c_void,
            info_size: u32,
        ) -> i32;
    }

    pub(super) fn open_file(root: &Path, rel: &Path) -> Result<File, CompatError> {
        let mut current = root.to_path_buf();
        let mut components = rel.components().peekable();
        while let Some(component) = components.next() {
            let Component::Normal(name) = component else {
                return Err(CompatError::OutOfBounds(rel_key(rel)));
            };
            current.push(name);
            let file = open_no_follow(&current)?;
            let attributes = attributes(&file)?;
            if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(CompatError::Invalid(format!(
                    "symlink component rejected: {}",
                    rel_key(rel)
                )));
            }
            if components.peek().is_some() {
                if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
                    return Err(CompatError::Invalid(format!(
                        "not a directory component: {}",
                        rel_key(rel)
                    )));
                }
            } else if attributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_DEVICE) != 0
                || !file
                    .metadata()
                    .map_err(|error| CompatError::io(rel, error))?
                    .is_file()
            {
                return Err(CompatError::Invalid(format!(
                    "not a regular file: {}",
                    rel_key(rel)
                )));
            } else {
                return Ok(file);
            }
        }
        Err(CompatError::OutOfBounds(rel_key(rel)))
    }

    fn open_no_follow(path: &Path) -> Result<File, CompatError> {
        OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|error| CompatError::io(path, error))
    }

    fn attributes(file: &File) -> Result<u32, CompatError> {
        let mut info = FileAttributeTagInfo {
            file_attributes: 0,
            reparse_tag: 0,
        };
        let ok = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FILE_ATTRIBUTE_TAG_INFO,
                (&mut info as *mut FileAttributeTagInfo).cast(),
                std::mem::size_of::<FileAttributeTagInfo>() as u32,
            )
        };
        if ok == 0 {
            return Err(CompatError::io(
                PathBuf::from("<opened-handle>"),
                std::io::Error::last_os_error(),
            ));
        }
        Ok(info.file_attributes)
    }
}

/// 判断路径本身是否为 symlink（不跟随；用于 apply 拒绝输出目录 / 目标 symlink）。
pub(crate) fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .ok()
        .is_some_and(|meta| meta.file_type().is_symlink())
}

/// 原子写：先写同目录临时文件再 rename，期间拒绝目标被替换为 symlink。
/// 临时文件名带进程 id + 计数 + 随机后缀，避免并发 apply 互相覆盖。
pub(crate) fn atomic_write(target: &Path, data: &[u8]) -> Result<(), CompatError> {
    use std::io::Write;
    if is_symlink(target) {
        return Err(CompatError::UnsafeTarget(rel_key(target)));
    }
    let directory = target
        .parent()
        .ok_or_else(|| CompatError::UnsafeTarget(rel_key(target)))?;
    let stem = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "compat".to_string());
    let tmp = unique_tmp_path(directory, &stem);
    {
        let mut handle = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|error| CompatError::io(&tmp, error))?;
        handle
            .write_all(data)
            .map_err(|error| CompatError::io(&tmp, error))?;
        handle
            .sync_all()
            .map_err(|error| CompatError::io(&tmp, error))?;
    }
    // rename 前再次确认目标未被替换为 symlink（缩小窗口）。
    if is_symlink(target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(CompatError::UnsafeTarget(rel_key(target)));
    }
    match std::fs::rename(&tmp, target) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&tmp);
            Err(CompatError::io(target, error))
        }
    }
}

static ATOMIC_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn unique_tmp_path(directory: &Path, stem: &str) -> PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos() as u64)
        .unwrap_or(0);
    loop {
        let attempt = ATOMIC_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let candidate = directory.join(format!(
            ".{stem}.{}.{:x}{:x}.tmp",
            std::process::id(),
            attempt,
            nanos
        ));
        if !candidate.exists() {
            return candidate;
        }
    }
}
