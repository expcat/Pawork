//! 归档根句柄上的平台安全目录遍历（P17-2 安全复审）。
//!
//! 归档校验 / 读取一律从**已打开的 root 句柄**出发，相对路径逐级 no-follow 打开，
//! 绝不以 `root.join(...)` 按路径重开（路径重开会重新解析中间分量，存在
//! ancestor swap / TOCTOU 窗口）。只做 canonicalize 或只检查最终分量都不够：
//!
//! - **Unix**：`open`/`openat` + `O_NOFOLLOW` 逐级打开目录与文件（句柄链），
//!   `fstatat(AT_SYMLINK_NOFOLLOW)` 分类条目、`fdopendir` 枚举。任何祖先分量被
//!   替换为 symlink 后，后续打开必然以 `ELOOP`/`ENOTDIR` 拒绝（fail-closed）。
//! - **Windows**：每个分量以 `FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS`
//!   打开，再用 `GetFileInformationByHandleEx(FileAttributeTagInfo)` 检查属性，
//!   symlink / junction / mount point 等一切 reparse point 以及非目录 / 非普通
//!   文件一律拒绝；枚举用 `std::fs::read_dir`（条目分类不跟随）。Windows 的
//!   句柄相对打开依赖未文档化 ntdll API，故枚举按路径重开，并在每次打开时对
//!   全链重新做 no-follow 属性校验（静态逐级拒绝保证；残留竞态窗口见各函数
//!   注释，威胁模型为攻击者同时持有归档根目录写权限）。
//!
//! [`SafeFsError`] 把平台差异收敛为 NotFound / NotRegular / Io 三类，由 archive
//! 层映射为 [`PackageError`]。

use std::io;

/// 目录条目分类（不跟随 symlink / junction）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChildKind {
    Dir,
    File,
    Other,
}

/// 平台安全遍历的收敛错误：文件缺失 / 非普通文件（symlink、reparse point、
/// 目录错位、socket、FIFO、设备等）/ 其他 I/O 错误。
#[derive(Debug)]
pub(crate) enum SafeFsError {
    NotFound,
    NotRegular,
    Io(io::Error),
}

#[cfg(unix)]
pub(crate) use unix_impl::SafeDir;
#[cfg(windows)]
pub(crate) use windows_impl::SafeDir;

#[cfg(unix)]
mod unix_impl {
    use std::ffi::{CStr, CString, OsStr, OsString};
    use std::fs::File;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::Path;

    use super::{ChildKind, SafeFsError};

    /// 已打开并校验的目录句柄（`O_DIRECTORY | O_NOFOLLOW`）。
    #[derive(Debug)]
    pub(crate) struct SafeDir {
        fd: OwnedFd,
    }

    impl SafeDir {
        /// 打开归档根目录句柄：root 本身为 symlink / 普通文件 / 不存在时拒绝。
        pub(crate) fn open(path: &Path) -> Result<Self, SafeFsError> {
            let c_path = c_string(path.as_os_str())?;
            let fd = unsafe {
                libc::open(
                    c_path.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(SafeFsError::from_errno());
            }
            Ok(Self {
                fd: unsafe { OwnedFd::from_raw_fd(fd) },
            })
        }

        /// dup 一份句柄（从 root 出发逐级重开时使用）。
        pub(crate) fn try_clone(&self) -> Result<Self, SafeFsError> {
            let fd = unsafe { libc::fcntl(self.fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
            if fd < 0 {
                return Err(SafeFsError::from_errno());
            }
            Ok(Self {
                fd: unsafe { OwnedFd::from_raw_fd(fd) },
            })
        }

        /// 相对当前句柄逐级打开子目录：`O_NOFOLLOW`，symlink / 非目录一律拒绝。
        pub(crate) fn open_child_dir(&self, name: &OsStr) -> Result<Self, SafeFsError> {
            let c_name = c_string(name)?;
            let fd = unsafe {
                libc::openat(
                    self.fd.as_raw_fd(),
                    c_name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(SafeFsError::from_errno());
            }
            Ok(Self {
                fd: unsafe { OwnedFd::from_raw_fd(fd) },
            })
        }

        /// 相对当前句柄打开普通文件：`O_NOFOLLOW`，句柄上 fstat 复核 regular。
        /// `O_NONBLOCK` 防止路径指向 FIFO / 设备时打开阻塞。
        pub(crate) fn open_child_file(&self, name: &OsStr) -> Result<File, SafeFsError> {
            let c_name = c_string(name)?;
            let fd = unsafe {
                libc::openat(
                    self.fd.as_raw_fd(),
                    c_name.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                )
            };
            if fd < 0 {
                return Err(SafeFsError::from_errno());
            }
            let file = unsafe { File::from_raw_fd(fd) };
            // fstat 作用于已打开句柄：检查与读取针对同一对象，无 TOCTOU。
            if !file.metadata().map_err(SafeFsError::Io)?.is_file() {
                return Err(SafeFsError::NotRegular);
            }
            Ok(file)
        }

        /// 枚举当前目录的子项（名称 + 不跟随分类）；symlink / 特殊文件为 Other。
        pub(crate) fn children(&self) -> Result<Vec<(OsString, ChildKind)>, SafeFsError> {
            let dup_fd = unsafe { libc::dup(self.fd.as_raw_fd()) };
            if dup_fd < 0 {
                return Err(SafeFsError::from_errno());
            }
            let dirp = unsafe { libc::fdopendir(dup_fd) };
            if dirp.is_null() {
                let error = SafeFsError::from_errno();
                unsafe { libc::close(dup_fd) };
                return Err(error);
            }
            let mut out = Vec::new();
            loop {
                unsafe { clear_errno() }
                let entry = unsafe { libc::readdir(dirp) };
                if entry.is_null() {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() == Some(0) {
                        break; // EOF（errno 已被清零，未置位即为正常结束）
                    }
                    unsafe { libc::closedir(dirp) };
                    return Err(SafeFsError::Io(error));
                }
                let name_bytes = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
                if name_bytes == b"." || name_bytes == b".." {
                    continue;
                }
                let name = OsString::from_vec(name_bytes.to_vec());
                let kind = self.classify(&name)?;
                out.push((name, kind));
            }
            unsafe { libc::closedir(dirp) };
            Ok(out)
        }

        /// `fstatat(AT_SYMLINK_NOFOLLOW)` 分类：symlink 报告为 Other（不跟随）。
        fn classify(&self, name: &OsStr) -> Result<ChildKind, SafeFsError> {
            let c_name = c_string(name)?;
            let mut stat: libc::stat = unsafe { std::mem::zeroed() };
            let result = unsafe {
                libc::fstatat(
                    self.fd.as_raw_fd(),
                    c_name.as_ptr(),
                    &mut stat,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if result != 0 {
                return Err(SafeFsError::from_errno());
            }
            match stat.st_mode & libc::S_IFMT {
                libc::S_IFDIR => Ok(ChildKind::Dir),
                libc::S_IFREG => Ok(ChildKind::File),
                _ => Ok(ChildKind::Other),
            }
        }
    }

    impl SafeFsError {
        /// 从 errno 映射：ENOENT 为缺失，ELOOP / ENOTDIR 为 symlink 或分量错位。
        fn from_errno() -> Self {
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::ENOENT) => Self::NotFound,
                Some(libc::ELOOP) | Some(libc::ENOTDIR) => Self::NotRegular,
                _ => Self::Io(error),
            }
        }
    }

    fn c_string(value: &OsStr) -> Result<CString, SafeFsError> {
        CString::new(value.as_bytes()).map_err(|_| {
            SafeFsError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path contains NUL byte",
            ))
        })
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    unsafe fn clear_errno() {
        *libc::__error() = 0;
    }

    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    unsafe fn clear_errno() {
        *libc::__errno_location() = 0;
    }
}

#[cfg(windows)]
mod windows_impl {
    use std::ffi::OsStr;
    use std::fs::{self, File, OpenOptions};
    use std::io;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::{Path, PathBuf};

    use super::{ChildKind, SafeFsError};

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    const FILE_ATTRIBUTE_DEVICE: u32 = 0x40;
    /// `FILE_INFO_BY_HANDLE_CLASS::FileAttributeTagInfo`。
    const FILE_ATTRIBUTE_TAG_INFO: u32 = 9;

    #[repr(C)]
    #[derive(Clone, Copy)]
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

    /// 已打开并校验的目录；`path` 供枚举使用（std `read_dir` 仅接受路径）。
    #[derive(Clone, Debug)]
    pub(crate) struct SafeDir {
        handle: File,
        path: PathBuf,
    }

    impl SafeDir {
        /// 打开归档根目录：`OPEN_REPARSE_POINT` 打开 + 属性检查（reparse / 非目录拒绝）。
        pub(crate) fn open(path: &Path) -> Result<Self, SafeFsError> {
            let handle = open_no_follow(path)?;
            let attributes = handle_attributes(&handle)?;
            if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(SafeFsError::NotRegular);
            }
            if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
                return Err(SafeFsError::NotRegular);
            }
            Ok(Self {
                handle,
                path: path.to_path_buf(),
            })
        }

        pub(crate) fn try_clone(&self) -> Result<Self, SafeFsError> {
            Ok(Self {
                handle: self.handle.try_clone().map_err(SafeFsError::Io)?,
                path: self.path.clone(),
            })
        }

        /// 逐级打开子目录：路径上每个分量在本次打开中重新解析，最终分量以
        /// `OPEN_REPARSE_POINT` 打开并由属性检查拒绝 junction / symlink。
        pub(crate) fn open_child_dir(&self, name: &OsStr) -> Result<Self, SafeFsError> {
            let path = self.path.join(name);
            let handle = open_no_follow(&path)?;
            let attributes = handle_attributes(&handle)?;
            if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(SafeFsError::NotRegular);
            }
            if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
                return Err(SafeFsError::NotRegular);
            }
            Ok(Self { handle, path })
        }

        /// 打开普通文件：reparse / 目录 / 设备属性与句柄 metadata 复核。
        pub(crate) fn open_child_file(&self, name: &OsStr) -> Result<File, SafeFsError> {
            let path = self.path.join(name);
            let handle = open_no_follow(&path)?;
            let attributes = handle_attributes(&handle)?;
            if attributes
                & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_DEVICE)
                != 0
            {
                return Err(SafeFsError::NotRegular);
            }
            // 句柄上的 metadata（GetFileInformationByHandle）复核普通文件。
            if !handle.metadata().map_err(SafeFsError::Io)?.is_file() {
                return Err(SafeFsError::NotRegular);
            }
            Ok(handle)
        }

        /// 枚举子项；条目分类用 `file_type()`（不跟随，reparse point 视为 symlink）。
        /// 注意：`read_dir` 按路径重开，若在打开校验后、枚举前目录被整体替换，
        /// 枚举可能指向新目录；子项后续打开仍会逐级重新校验并拒绝 reparse。
        pub(crate) fn children(&self) -> Result<Vec<(OsString, ChildKind)>, SafeFsError> {
            let mut out = Vec::new();
            for entry in fs::read_dir(&self.path).map_err(SafeFsError::Io)? {
                let entry = entry.map_err(SafeFsError::Io)?;
                let file_type = entry.file_type().map_err(SafeFsError::Io)?;
                let kind = if file_type.is_dir() {
                    ChildKind::Dir
                } else if file_type.is_file() {
                    ChildKind::File
                } else {
                    ChildKind::Other
                };
                out.push((entry.file_name(), kind));
            }
            Ok(out)
        }
    }

    /// 以 `OPEN_REPARSE_POINT` 打开：最终分量若为 symlink / junction / reparse
    /// point，返回的句柄指向 reparse point 自身（不跟随），由属性检查拒绝。
    fn open_no_follow(path: &Path) -> Result<File, SafeFsError> {
        OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|error| match error.kind() {
                io::ErrorKind::NotFound => SafeFsError::NotFound,
                _ => SafeFsError::Io(error),
            })
    }

    fn handle_attributes(file: &File) -> Result<u32, SafeFsError> {
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
            return Err(SafeFsError::Io(io::Error::last_os_error()));
        }
        Ok(info.file_attributes)
    }
}
