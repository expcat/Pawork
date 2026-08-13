//! Package 归档读写与内容寻址完整性校验（P17-2）。
//!
//! 归档形态：一个目录树 + `package.toml`（manifest）+ `contents.b3`（内容寻址清单）。
//! `contents.b3` 为每归档内普通文件记录 POSIX 相对路径与 blake3 摘要；读取时逐文件
//! 重算并比对，损坏 / 篡改可检测。内容清单自身不列入（避免自引用），但 manifest
//! 与所有资源文件均纳入校验。
//!
//! 这一层为 [P17-3](../../plan/P17-3-plugin-marketplace.md) marketplace 的签名 / 校验
//! 提供基础：marketplace 在本摘要之上叠加签名验证，package 格式只保证内容完整性。
//!
//! # 安全模型（2026-08 安全复审）
//!
//! 校验 / 读取一律从**已打开的归档根句柄**出发（见 [`crate::fs_safe`]），相对路径
//! 逐级 no-follow 打开，绝不以 `root.join(...)` 按路径重开：
//!
//! - Unix：`openat` + `O_NOFOLLOW` 句柄链遍历，防 ancestor swap；
//! - Windows：每级以 `FILE_FLAG_OPEN_REPARSE_POINT` 打开并拒绝一切 symlink /
//!   junction / reparse point。
//!
//! [`PackageArchive`] 持有该根句柄，`read_file` 提供逐级 no-follow + 摘要复核的
//! 安全读取（冲突检测等下游一律经它读子 manifest，symlink 替换 / 篡改 fail-closed）。
//!
//! # 资源上限（2026-08 安全复审）
//!
//! 不可信归档的读取 / 哈希 / 枚举全程受资源上限约束（[`MAX_CONTENT_MANIFEST_BYTES`]、
//! [`MAX_PACKAGE_MANIFEST_BYTES`]、[`MAX_FILE_BYTES`]、[`MAX_TOTAL_BYTES`]、
//! [`MAX_ENTRY_COUNT`]，以及 [`crate::scope::MAX_PATH_DEPTH`] /
//! [`crate::scope::MAX_PATH_BYTES`]）：检查先于分配与完整读取，超限即返回
//! [`PackageError::ResourceLimit`]（fail-closed），避免无界内存 / CPU 放大。

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path};
use std::sync::Arc;

use crate::error::PackageError;
use crate::fs_safe::{ChildKind, SafeDir, SafeFsError};
use crate::manifest::{PackageManifest, MANIFEST_FILE_NAME};
use crate::scope::PackageRelativePath;

/// 内容寻址清单文件名（blake3 摘要表）。
pub const CONTENT_MANIFEST_FILE_NAME: &str = "contents.b3";

/// `contents.b3` 大小上限（1 MiB）：内容清单在解析前会被完整读入内存，超限
/// 分配 / 读取必须先失败（fail-closed）。
pub const MAX_CONTENT_MANIFEST_BYTES: u64 = 1 << 20;

/// `package.toml` 大小上限（256 KiB）。
pub const MAX_PACKAGE_MANIFEST_BYTES: u64 = 256 << 10;

/// 归档内单个文件的大小上限（64 MiB）。
pub const MAX_FILE_BYTES: u64 = 64 << 20;

/// 归档内全部内容条目的总字节上限（256 MiB）。
pub const MAX_TOTAL_BYTES: u64 = 256 << 20;

/// 归档内容条目数量上限。
pub const MAX_ENTRY_COUNT: usize = 4096;

/// 归档读取 / 校验 / 写入使用的资源上限。对不可信归档一律 fail-closed。
///
/// 默认值取自本模块公开常量；测试与后续 marketplace 策略可逐项覆盖。
#[derive(Clone, Copy, Debug)]
struct ArchiveLimits {
    content_manifest_bytes: u64,
    package_manifest_bytes: u64,
    file_bytes: u64,
    total_bytes: u64,
    entries: usize,
    path_depth: usize,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            content_manifest_bytes: MAX_CONTENT_MANIFEST_BYTES,
            package_manifest_bytes: MAX_PACKAGE_MANIFEST_BYTES,
            file_bytes: MAX_FILE_BYTES,
            total_bytes: MAX_TOTAL_BYTES,
            entries: MAX_ENTRY_COUNT,
            path_depth: crate::scope::MAX_PATH_DEPTH,
        }
    }
}

/// 单个归档内容条目。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentEntry {
    pub path: PackageRelativePath,
    pub blake3_hex: String,
    pub size: u64,
}

/// 已读取并校验完整性的归档。
#[derive(Clone, Debug)]
pub struct PackageArchive {
    pub manifest: PackageManifest,
    pub entries: Vec<ContentEntry>,
    /// 已打开并校验的归档根句柄：后续相对读取（如冲突检测读子 manifest）一律经
    /// 此句柄逐级 no-follow 打开，杜绝按路径重开导致的 ancestor swap。
    root: Arc<SafeDir>,
}

impl PackageArchive {
    /// 经已持有的归档根句柄逐级 no-follow 读取归档内文件，并复核其 blake3 摘要。
    ///
    /// 读取受单文件大小上限约束：分配上限取已校验条目大小，文件在验证后膨胀
    /// 时最多读入 cap+1 字节并按摘要不匹配失败（fail-closed），不做无界分配。
    ///
    /// 文件在归档验证后被替换为 symlink、被篡改或未登记时一律失败（fail-closed）。
    pub fn read_file(&self, path: &PackageRelativePath) -> Result<Vec<u8>, PackageError> {
        let expected = self
            .entries
            .iter()
            .find(|entry| entry.path == *path)
            .ok_or_else(|| PackageError::MissingFile(path.as_path().to_path_buf()))?;
        // 错误上下文使用相对路径（read_file 面向归档内路径）。
        let file = open_relative_file(&self.root, Path::new(""), path.as_path())?;
        // 受限读取：分配上限 = 已校验条目大小，另受单文件上限封顶，防无界分配。
        let cap = expected.size.min(MAX_FILE_BYTES);
        let mut bytes = Vec::with_capacity(cap as usize);
        file.take(cap.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(PackageError::ArchiveIo)?;
        let actual_hex = blake3::hash(&bytes).to_hex().to_string();
        if !actual_hex.eq_ignore_ascii_case(&expected.blake3_hex) {
            return Err(PackageError::HashMismatch {
                path: path.as_path().to_path_buf(),
                expected: expected.blake3_hex.clone(),
                found: actual_hex,
            });
        }
        Ok(bytes)
    }
}

/// 写入归档：序列化 manifest 到 `package.toml`，扫描归档内所有普通文件（排除
/// `contents.b3`）并写入 `contents.b3`。`root` 必须已包含 package 的资源文件。
pub fn write_archive(root: &Path, manifest: &PackageManifest) -> Result<(), PackageError> {
    write_archive_with(root, manifest, &ArchiveLimits::default())
}

fn write_archive_with(
    root: &Path,
    manifest: &PackageManifest,
    limits: &ArchiveLimits,
) -> Result<(), PackageError> {
    // 根句柄：root 必须是真实目录（非 symlink），为遍历与哈希提供一致基点。
    let root_handle = SafeDir::open(root).map_err(|error| map_safe_error(root, error))?;
    manifest.validate()?;
    let manifest_path = root.join(MANIFEST_FILE_NAME);
    fs::write(&manifest_path, manifest.to_toml_string()?).map_err(PackageError::ArchiveIo)?;

    let mut entries = collect_entries(&root_handle, root, limits)?;
    entries.sort_by_key(|entry| entry.0.clone());
    let mut rendered = String::new();
    for (posix, hash, size) in &entries {
        rendered.push_str(hash);
        rendered.push_str("  ");
        rendered.push_str(posix);
        rendered.push('\n');
        // size 仅记录在内存条目里；摘要文件保持 `<hex>  <path>` 以便审计 diff。
        let _ = size;
    }
    fs::write(root.join(CONTENT_MANIFEST_FILE_NAME), rendered).map_err(PackageError::ArchiveIo)?;
    Ok(())
}

/// 读取并校验归档完整性。返回 manifest 与已校验的内容条目；任一文件的 blake3
/// 不匹配或缺失即返回错误（损坏 / 篡改可检测）。
pub fn read_archive(root: &Path) -> Result<PackageArchive, PackageError> {
    read_archive_with(root, &ArchiveLimits::default())
}

fn read_archive_with(root: &Path, limits: &ArchiveLimits) -> Result<PackageArchive, PackageError> {
    let root_handle = SafeDir::open(root).map_err(|error| map_safe_error(root, error))?;
    let entries = verify_archive_from_handle(&root_handle, root, limits)?;
    // 二次打开后必须重新绑定已验证条目：同名普通文件可在完整性扫描与解析之间
    // 被原子替换，O_NOFOLLOW 只能拒绝 symlink，不能证明两次打开是同一对象。
    let manifest = read_manifest(&root_handle, root, limits, &entries)?;
    verify_referenced_files(&manifest, &entries)?;
    Ok(PackageArchive {
        manifest,
        entries,
        root: Arc::new(root_handle),
    })
}

/// 仅校验归档完整性（不解析 manifest 引用），返回已校验条目。
pub fn verify_archive(root: &Path) -> Result<Vec<ContentEntry>, PackageError> {
    verify_archive_with(root, &ArchiveLimits::default())
}

fn verify_archive_with(
    root: &Path,
    limits: &ArchiveLimits,
) -> Result<Vec<ContentEntry>, PackageError> {
    let root_handle = SafeDir::open(root).map_err(|error| map_safe_error(root, error))?;
    verify_archive_from_handle(&root_handle, root, limits)
}

fn verify_archive_from_handle(
    root: &SafeDir,
    root_path: &Path,
    limits: &ArchiveLimits,
) -> Result<Vec<ContentEntry>, PackageError> {
    let mut manifest_file =
        open_relative_file(root, root_path, Path::new(CONTENT_MANIFEST_FILE_NAME))?;
    // 按字节上限读取内容清单：最多读入上限 + 1 字节，超限即失败（fail-closed）。
    let text = read_string_limited(
        &mut manifest_file,
        limits.content_manifest_bytes,
        "contents.b3 size",
    )?;
    let mut entries = Vec::new();
    let mut declared_paths = BTreeSet::new();
    let mut total_bytes = 0u64;
    for (line_index, line) in text.lines().enumerate() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if entries.len() >= limits.entries {
            return Err(PackageError::ResourceLimit {
                resource: "content entry count",
                limit: limits.entries as u64,
                found: (entries.len() as u64) + 1,
            });
        }
        let (hash, posix) = line.split_once("  ").ok_or_else(|| {
            PackageError::ManifestToml(format!("contents.b3 line is malformed: {line}"))
        })?;
        let hash = hash.trim();
        let posix = posix.trim();
        validate_blake3_hex(hash, line_index + 1)?;
        let relative = PackageRelativePath::new(posix)?;
        let canonical_path = relative.to_posix_string();
        if canonical_path == CONTENT_MANIFEST_FILE_NAME {
            return Err(PackageError::NotRegularFile(
                root_path.join(CONTENT_MANIFEST_FILE_NAME),
            ));
        }
        if !declared_paths.insert(canonical_path) {
            return Err(PackageError::DuplicateContentPath(
                relative.as_path().to_path_buf(),
            ));
        }
        let absolute = root_path.join(relative.as_path());
        let mut file = open_relative_file(root, root_path, relative.as_path())?;
        let size = verify_file_hash(&mut file, &absolute, hash, limits, total_bytes)?;
        total_bytes = total_bytes.saturating_add(size);
        entries.push(ContentEntry {
            path: relative,
            blake3_hex: hash.to_string(),
            size,
        });
    }

    if !declared_paths.contains(MANIFEST_FILE_NAME) {
        return Err(PackageError::UnlistedFile(
            root_path.join(MANIFEST_FILE_NAME),
        ));
    }

    let disk_paths = collect_disk_file_paths(root, root_path, limits)?;
    if let Some(unlisted) = disk_paths.difference(&declared_paths).next() {
        return Err(PackageError::UnlistedFile(root_path.join(unlisted)));
    }
    // Listed-but-missing paths have already failed in verify_file_hash. Keep
    // this equality check explicit so the closed-manifest invariant remains
    // visible if verification order changes later.
    if let Some(missing) = declared_paths.difference(&disk_paths).next() {
        return Err(PackageError::MissingFile(root_path.join(missing)));
    }
    Ok(entries)
}

fn validate_blake3_hex(value: &str, line: usize) -> Result<(), PackageError> {
    if value.len() != blake3::OUT_LEN * 2 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(PackageError::InvalidContentHash {
            line,
            value: value.to_string(),
        });
    }
    Ok(())
}

fn read_manifest(
    root: &SafeDir,
    root_path: &Path,
    limits: &ArchiveLimits,
    entries: &[ContentEntry],
) -> Result<PackageManifest, PackageError> {
    let manifest_path = root_path.join(MANIFEST_FILE_NAME);
    let expected = entries
        .iter()
        .find(|entry| entry.path.as_path() == Path::new(MANIFEST_FILE_NAME))
        .ok_or_else(|| PackageError::MissingFile(manifest_path.clone()))?;
    let mut manifest_file = root
        .open_child_file(OsStr::new(MANIFEST_FILE_NAME))
        .map_err(|error| map_safe_error(&manifest_path, error))?;
    // 在本次打开的同一句柄上受限读取并复核摘要，再解析已复核的字节。
    let text = read_string_limited(
        &mut manifest_file,
        limits.package_manifest_bytes.min(expected.size),
        "package.toml size",
    )?;
    let found = blake3::hash(text.as_bytes()).to_hex().to_string();
    if !found.eq_ignore_ascii_case(&expected.blake3_hex) {
        return Err(PackageError::HashMismatch {
            path: manifest_path,
            expected: expected.blake3_hex.clone(),
            found,
        });
    }
    PackageManifest::from_toml_str(&text)
}

/// 读取已打开文件并校验其 blake3 摘要；返回文件大小。文件以 no-follow 句柄链
/// 打开且 regular 检查作用于已打开句柄，路径在检查后被替换为 symlink / 特殊
/// 文件时不会读入（TOCTOU 消除）。
/// 读取过程受单文件与累计字节上限约束，超限立即失败（fail-closed）。
fn verify_file_hash(
    file: &mut File,
    path: &Path,
    expected_hex: &str,
    limits: &ArchiveLimits,
    used_total: u64,
) -> Result<u64, PackageError> {
    let (hash, size) = hash_file_bounded(file, limits, used_total)?;
    let actual_hex = hash.to_hex().to_string();
    if !actual_hex.eq_ignore_ascii_case(expected_hex) {
        return Err(PackageError::HashMismatch {
            path: path.to_path_buf(),
            expected: expected_hex.to_string(),
            found: actual_hex,
        });
    }
    Ok(size)
}

/// 分块读取并哈希文件，逐块检查单文件与累计字节上限：超限立即失败
/// （fail-closed），绝不读入 / 分配超过上限的字节。
fn hash_file_bounded(
    file: &mut File,
    limits: &ArchiveLimits,
    used_total: u64,
) -> Result<(blake3::Hash, u64), PackageError> {
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer).map_err(PackageError::ArchiveIo)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
        if size > limits.file_bytes {
            return Err(PackageError::ResourceLimit {
                resource: "single file size",
                limit: limits.file_bytes,
                found: size,
            });
        }
        if used_total.saturating_add(size) > limits.total_bytes {
            return Err(PackageError::ResourceLimit {
                resource: "total archive size",
                limit: limits.total_bytes,
                found: used_total.saturating_add(size),
            });
        }
    }
    Ok((hasher.finalize(), size))
}

/// 按字节上限完整读取文本文件：最多读入 max_bytes + 1 字节，超限即失败
/// （fail-closed），检查前不做无界分配 / 读取；非 UTF8 按归档数据损坏处理。
fn read_string_limited(
    file: &mut File,
    max_bytes: u64,
    resource: &'static str,
) -> Result<String, PackageError> {
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(PackageError::ArchiveIo)?;
    if bytes.len() as u64 > max_bytes {
        return Err(PackageError::ResourceLimit {
            resource,
            limit: max_bytes,
            found: bytes.len() as u64,
        });
    }
    String::from_utf8(bytes)
        .map_err(|error| PackageError::ArchiveIo(io::Error::new(io::ErrorKind::InvalidData, error)))
}

/// 从 root 句柄逐级 no-follow 打开相对文件（中间分量 openat 目录、最终分量
/// openat 文件），并以 root 路径构造错误上下文。
fn open_relative_file(
    root: &SafeDir,
    root_path: &Path,
    relative: &Path,
) -> Result<File, PackageError> {
    let parts: Vec<&OsStr> = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(OsStr::new(part)),
            _ => None,
        })
        .collect();
    let (last, parents) = parts
        .split_last()
        .ok_or_else(|| PackageError::PathEscape(root_path.to_path_buf()))?;
    let mut current = root
        .try_clone()
        .map_err(|error| map_safe_error(root_path, error))?;
    let mut current_path = root_path.to_path_buf();
    for part in parents {
        current_path.push(part);
        current = current
            .open_child_dir(part)
            .map_err(|error| map_safe_error(&current_path, error))?;
    }
    current_path.push(last);
    current
        .open_child_file(last)
        .map_err(|error| map_safe_error(&current_path, error))
}

fn map_safe_error(path: &Path, error: SafeFsError) -> PackageError {
    match error {
        SafeFsError::NotFound => PackageError::MissingFile(path.to_path_buf()),
        SafeFsError::NotRegular => PackageError::NotRegularFile(path.to_path_buf()),
        SafeFsError::Io(error) => PackageError::ArchiveIo(error),
    }
}

/// POSIX 相对路径的分量数上限检查：防止深路径遍历与递归 / 分配放大。
fn check_posix_depth(posix: &str, limits: &ArchiveLimits) -> Result<(), PackageError> {
    let depth = posix.split('/').count();
    if depth > limits.path_depth {
        return Err(PackageError::ResourceLimit {
            resource: "path depth",
            limit: limits.path_depth as u64,
            found: depth as u64,
        });
    }
    Ok(())
}

/// Collect the exact set of ordinary archive files, excluding contents.b3.
/// Symlinks and special files are rejected even when absent from the manifest.
fn collect_disk_file_paths(
    root: &SafeDir,
    root_path: &Path,
    limits: &ArchiveLimits,
) -> Result<BTreeSet<String>, PackageError> {
    let mut out = BTreeSet::new();
    collect_disk_paths(root, root_path, "", &mut out, limits)?;
    Ok(out)
}

fn collect_disk_paths(
    dir: &SafeDir,
    dir_path: &Path,
    prefix: &str,
    out: &mut BTreeSet<String>,
    limits: &ArchiveLimits,
) -> Result<(), PackageError> {
    let mut children = dir
        .children()
        .map_err(|error| map_safe_error(dir_path, error))?;
    children.sort_by_key(|entry| entry.0.clone());
    for (name, kind) in children {
        let child_path = dir_path.join(&name);
        let posix_name = name
            .to_str()
            .ok_or_else(|| PackageError::PathEscape(child_path.clone()))?;
        let posix = if prefix.is_empty() {
            posix_name.to_string()
        } else {
            format!("{prefix}/{posix_name}")
        };
        // 深度上限：递归前约束，防深目录链的栈放大。
        check_posix_depth(&posix, limits)?;
        match kind {
            ChildKind::Dir => {
                let sub = dir
                    .open_child_dir(&name)
                    .map_err(|error| map_safe_error(&child_path, error))?;
                collect_disk_paths(&sub, &child_path, &posix, out, limits)?;
            }
            ChildKind::File => {
                // 校验相对路径不变量（拒绝 `..` / 绝对段）。
                let _ = PackageRelativePath::new(Path::new(&posix))?;
                if posix != CONTENT_MANIFEST_FILE_NAME {
                    if out.len() >= limits.entries {
                        return Err(PackageError::ResourceLimit {
                            resource: "content entry count",
                            limit: limits.entries as u64,
                            found: (out.len() as u64) + 1,
                        });
                    }
                    out.insert(posix);
                }
            }
            ChildKind::Other => return Err(PackageError::NotRegularFile(child_path)),
        }
    }
    Ok(())
}

/// 递归收集归档内所有普通文件的 (posix_path, blake3_hex, size)，排除内容清单自身。
fn collect_entries(
    root: &SafeDir,
    root_path: &Path,
    limits: &ArchiveLimits,
) -> Result<Vec<(String, String, u64)>, PackageError> {
    let mut out = Vec::new();
    let mut total_bytes = 0u64;
    walk(root, root_path, "", &mut out, limits, &mut total_bytes)?;
    Ok(out)
}

fn walk(
    dir: &SafeDir,
    dir_path: &Path,
    prefix: &str,
    out: &mut Vec<(String, String, u64)>,
    limits: &ArchiveLimits,
    total_bytes: &mut u64,
) -> Result<(), PackageError> {
    let mut children = dir
        .children()
        .map_err(|error| map_safe_error(dir_path, error))?;
    children.sort_by_key(|entry| entry.0.clone());
    for (name, kind) in children {
        let child_path = dir_path.join(&name);
        let posix_name = name
            .to_str()
            .ok_or_else(|| PackageError::PathEscape(child_path.clone()))?;
        let posix = if prefix.is_empty() {
            posix_name.to_string()
        } else {
            format!("{prefix}/{posix_name}")
        };
        // 深度上限：递归前约束，防深目录链的栈放大。
        check_posix_depth(&posix, limits)?;
        // 校验相对路径不变量（拒绝 `..` / 绝对段）。
        let _ = PackageRelativePath::new(Path::new(&posix))?;
        if kind == ChildKind::Dir {
            let sub = dir
                .open_child_dir(&name)
                .map_err(|error| map_safe_error(&child_path, error))?;
            walk(&sub, &child_path, &posix, out, limits, total_bytes)?;
        } else if kind == ChildKind::File {
            if posix == CONTENT_MANIFEST_FILE_NAME {
                continue;
            }
            if out.len() >= limits.entries {
                return Err(PackageError::ResourceLimit {
                    resource: "content entry count",
                    limit: limits.entries as u64,
                    found: (out.len() as u64) + 1,
                });
            }
            let mut file = dir
                .open_child_file(&name)
                .map_err(|error| map_safe_error(&child_path, error))?;
            let (hash, size) = hash_file_bounded(&mut file, limits, *total_bytes)?;
            *total_bytes = total_bytes.saturating_add(size);
            let hex = hash.to_hex().to_string();
            out.push((posix, hex, size));
        } else {
            // Writing an archive is strict as well: never silently omit a
            // symlink, socket, FIFO, device, or other special file.
            return Err(PackageError::NotRegularFile(child_path));
        }
    }
    Ok(())
}

/// 校验 manifest 中以路径引用的子资源（skills / agents / hooks / lsp + entrypoint）
/// 都存在于已校验的内容条目中。
fn verify_referenced_files(
    manifest: &PackageManifest,
    entries: &[ContentEntry],
) -> Result<(), PackageError> {
    // 引用路径可能是文件（精确匹配）或目录（skills 目录形态）；任一内容条目
    // 精确匹配或位于该路径前缀下即视为存在。
    let posix_paths: Vec<String> = entries
        .iter()
        .map(|entry| entry.path.to_posix_string())
        .collect();
    for resource_ref in manifest
        .skills
        .iter()
        .chain(manifest.agents.iter())
        .chain(manifest.hooks.iter())
        .chain(manifest.lsp.iter())
    {
        if let Some(path) = resource_ref.path() {
            if !path_has_content(path, &posix_paths) {
                return Err(PackageError::MissingFile(path.as_path().to_path_buf()));
            }
        }
    }
    if let Some(entrypoint) = &manifest.entrypoint {
        if !path_has_content(entrypoint, &posix_paths) {
            return Err(PackageError::MissingFile(
                entrypoint.as_path().to_path_buf(),
            ));
        }
    }
    Ok(())
}

/// 引用路径是否在归档中有内容：精确文件匹配，或目录前缀下至少一个文件。
fn path_has_content(path: &PackageRelativePath, posix_paths: &[String]) -> bool {
    let key = path.to_posix_string();
    let prefix = format!("{key}/");
    posix_paths
        .iter()
        .any(|entry| entry == &key || entry.starts_with(&prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ResourceRef;
    use crate::scope::{PackageId, PackageScope};
    use semver::Version;
    use std::io::Write;

    fn manifest_with_skill(skill_dir: &str) -> PackageManifest {
        PackageManifest {
            manifest_version: crate::PACKAGE_MANIFEST_VERSION,
            id: PackageId::new("acme.pkg").unwrap(),
            name: "ACME".into(),
            version: Version::new(1, 0, 0),
            license: None,
            description: None,
            entrypoint: None,
            scope: PackageScope::Global,
            dependencies: Vec::new(),
            skills: vec![ResourceRef::Path {
                path: PackageRelativePath::new(skill_dir).unwrap(),
            }],
            agents: Vec::new(),
            hooks: Vec::new(),
            mcp: Vec::new(),
            lsp: Vec::new(),
            monitors: Vec::new(),
        }
    }

    fn populated_archive() -> (tempfile::TempDir, PackageManifest) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let skill_dir = root.join("skills/search");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Search").unwrap();
        let manifest = manifest_with_skill("skills/search");
        write_archive(root, &manifest).unwrap();
        (temp, manifest)
    }

    /// 无资源引用的 manifest：构造最小归档（内容仅 package.toml 与测试文件）。
    fn manifest_no_resources() -> PackageManifest {
        PackageManifest {
            manifest_version: crate::PACKAGE_MANIFEST_VERSION,
            id: PackageId::new("acme.pkg").unwrap(),
            name: "ACME".into(),
            version: Version::new(1, 0, 0),
            license: None,
            description: None,
            entrypoint: None,
            scope: PackageScope::Global,
            dependencies: Vec::new(),
            skills: Vec::new(),
            agents: Vec::new(),
            hooks: Vec::new(),
            mcp: Vec::new(),
            lsp: Vec::new(),
            monitors: Vec::new(),
        }
    }

    #[test]
    fn default_limits_match_public_constants() {
        let limits = ArchiveLimits::default();
        assert_eq!(limits.content_manifest_bytes, MAX_CONTENT_MANIFEST_BYTES);
        assert_eq!(limits.package_manifest_bytes, MAX_PACKAGE_MANIFEST_BYTES);
        assert_eq!(limits.file_bytes, MAX_FILE_BYTES);
        assert_eq!(limits.total_bytes, MAX_TOTAL_BYTES);
        assert_eq!(limits.entries, MAX_ENTRY_COUNT);
        assert_eq!(limits.path_depth, crate::scope::MAX_PATH_DEPTH);
    }

    #[test]
    fn oversized_content_manifest_is_rejected_before_parsing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let oversized = vec![b'a'; (MAX_CONTENT_MANIFEST_BYTES + 1) as usize];
        fs::write(root.join(CONTENT_MANIFEST_FILE_NAME), &oversized).unwrap();

        let error = read_archive(root).unwrap_err();
        assert!(matches!(
            error,
            PackageError::ResourceLimit {
                resource: "contents.b3 size",
                limit: MAX_CONTENT_MANIFEST_BYTES,
                ..
            }
        ));
    }

    #[test]
    fn oversized_package_manifest_is_rejected_after_hash_verification() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // 用超长 description 把 package.toml 推过测试里收紧的上限。
        let mut manifest = manifest_no_resources();
        manifest.description = Some("x".repeat(8192));
        let toml = manifest.to_toml_string().unwrap();
        fs::write(root.join(MANIFEST_FILE_NAME), &toml).unwrap();
        let hash = blake3::hash(toml.as_bytes()).to_hex().to_string();
        fs::write(
            root.join(CONTENT_MANIFEST_FILE_NAME),
            format!("{hash}  {MANIFEST_FILE_NAME}\n"),
        )
        .unwrap();

        let limits = ArchiveLimits {
            package_manifest_bytes: 1024,
            ..Default::default()
        };
        let error = read_archive_with(root, &limits).unwrap_err();
        assert!(matches!(
            error,
            PackageError::ResourceLimit {
                resource: "package.toml size",
                ..
            }
        ));
    }

    #[test]
    fn entry_count_over_limit_is_rejected_on_read_and_write() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // package.toml + a.txt + b.txt = 3 个内容条目。
        fs::write(root.join("a.txt"), "a").unwrap();
        fs::write(root.join("b.txt"), "b").unwrap();
        let manifest = manifest_no_resources();

        let limits = ArchiveLimits {
            entries: 2,
            ..Default::default()
        };
        let error = write_archive_with(root, &manifest, &limits).unwrap_err();
        assert!(matches!(
            error,
            PackageError::ResourceLimit {
                resource: "content entry count",
                ..
            }
        ));

        // 先按默认上限写入合法归档，再用收紧的条目数上限读取 / 校验。
        write_archive(root, &manifest).unwrap();
        let error = verify_archive_with(root, &limits).unwrap_err();
        assert!(matches!(
            error,
            PackageError::ResourceLimit {
                resource: "content entry count",
                ..
            }
        ));
        let error = read_archive_with(root, &limits).unwrap_err();
        assert!(matches!(
            error,
            PackageError::ResourceLimit {
                resource: "content entry count",
                ..
            }
        ));
    }

    #[test]
    fn total_size_over_limit_is_rejected_during_hashing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(root.join("a.txt"), "aaaaaaaa").unwrap();
        fs::write(root.join("b.txt"), "bbbbbbbb").unwrap();
        let manifest = manifest_no_resources();

        let limits = ArchiveLimits {
            total_bytes: 10,
            ..Default::default()
        };
        let error = write_archive_with(root, &manifest, &limits).unwrap_err();
        assert!(matches!(
            error,
            PackageError::ResourceLimit {
                resource: "total archive size",
                ..
            }
        ));

        // 先按默认上限写入合法归档，再用收紧的总字节上限校验。
        write_archive(root, &manifest).unwrap();
        let error = verify_archive_with(root, &limits).unwrap_err();
        assert!(matches!(
            error,
            PackageError::ResourceLimit {
                resource: "total archive size",
                ..
            }
        ));
    }

    #[test]
    fn over_deep_path_in_content_manifest_is_rejected() {
        let (temp, _) = populated_archive();
        let contents_path = temp.path().join(CONTENT_MANIFEST_FILE_NAME);
        let mut contents = fs::read_to_string(&contents_path).unwrap();
        // 33 个分量：超出 MAX_PATH_DEPTH = 32。
        let deep_path = (0..=crate::scope::MAX_PATH_DEPTH)
            .map(|index| format!("d{index}"))
            .collect::<Vec<_>>()
            .join("/");
        contents.push_str(&format!("{}  {deep_path}\n", "a".repeat(64)));
        fs::write(&contents_path, contents).unwrap();

        let error = verify_archive(temp.path()).unwrap_err();
        assert!(matches!(
            error,
            PackageError::ResourceLimit {
                resource: "path depth",
                ..
            }
        ));
    }

    #[test]
    fn single_file_size_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // 恰好等于单文件上限：合法。
        fs::write(root.join("big.bin"), vec![0u8; MAX_FILE_BYTES as usize]).unwrap();
        let manifest = manifest_no_resources();
        write_archive(root, &manifest).unwrap();
        let archive = read_archive(root).expect("read archive at size boundary");
        assert_eq!(archive.entries.len(), 2);

        // 追加 1 字节：写入与读取路径都按单文件上限 fail-closed。
        fs::OpenOptions::new()
            .append(true)
            .open(root.join("big.bin"))
            .unwrap()
            .write_all(&[0u8])
            .unwrap();
        let error = write_archive(root, &manifest).unwrap_err();
        assert!(matches!(
            error,
            PackageError::ResourceLimit {
                resource: "single file size",
                ..
            }
        ));
        let error = read_archive(root).unwrap_err();
        assert!(matches!(
            error,
            PackageError::ResourceLimit {
                resource: "single file size",
                ..
            }
        ));
    }

    #[test]
    fn read_file_fails_closed_when_file_grows_after_verification() {
        let (temp, _) = populated_archive();
        let archive = read_archive(temp.path()).expect("read archive");
        let skill_file = temp.path().join("skills/search/SKILL.md");
        // 验证完成后追加内容：read_file 受限读取后必须按摘要不匹配 fail-closed。
        fs::OpenOptions::new()
            .append(true)
            .open(&skill_file)
            .unwrap()
            .write_all(b" (tampered)")
            .unwrap();
        let error = archive
            .read_file(&PackageRelativePath::new("skills/search/SKILL.md").unwrap())
            .unwrap_err();
        assert!(matches!(error, PackageError::HashMismatch { .. }));
    }

    #[test]
    fn write_and_read_round_trip_verifies_integrity() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let skill_dir = root.join("skills/search");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("manifest.toml"),
            "id='search'\nversion='1.0.0'",
        )
        .unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Search").unwrap();
        let manifest = manifest_with_skill("skills/search");

        write_archive(root, &manifest).expect("write");
        let archive = read_archive(root).expect("read");
        assert_eq!(archive.manifest.id.as_str(), "acme.pkg");
        // manifest.toml + SKILL.md + package.toml = 3 个内容条目（contents.b3 自身排除）。
        assert_eq!(archive.entries.len(), 3);
    }

    #[test]
    fn tampered_file_is_detected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let skill_dir = root.join("skills/search");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# original").unwrap();
        let manifest = manifest_with_skill("skills/search");
        write_archive(root, &manifest).unwrap();

        // 篡改资源文件。
        fs::write(skill_dir.join("SKILL.md"), "# tampered").unwrap();
        let err = read_archive(root).unwrap_err();
        assert!(matches!(err, PackageError::HashMismatch { .. }));
    }

    #[test]
    fn missing_referenced_file_is_detected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let manifest = manifest_with_skill("skills/missing");
        // 直接写 manifest + 内容清单，但 referenced 目录不存在。
        fs::write(
            root.join(MANIFEST_FILE_NAME),
            manifest.to_toml_string().unwrap(),
        )
        .unwrap();
        // 只写 contents.b3（package.toml 自身）。
        let package_toml = root.join(MANIFEST_FILE_NAME);
        let bytes = fs::read(&package_toml).unwrap();
        let hash = blake3::hash(&bytes).to_hex().to_string();
        fs::write(
            root.join(CONTENT_MANIFEST_FILE_NAME),
            format!("{hash}  {MANIFEST_FILE_NAME}\n"),
        )
        .unwrap();
        let err = read_archive(root).unwrap_err();
        assert!(matches!(err, PackageError::MissingFile(_)));
    }

    #[test]
    fn malformed_content_line_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(
            root.join(MANIFEST_FILE_NAME),
            manifest_with_skill("skills/x").to_toml_string().unwrap(),
        )
        .unwrap();
        fs::write(root.join(CONTENT_MANIFEST_FILE_NAME), "noseparator\n").unwrap();
        let err = read_archive(root).unwrap_err();
        assert!(err.to_string().contains("malformed"));
    }

    #[test]
    fn duplicate_content_path_is_rejected() {
        let (temp, _) = populated_archive();
        let contents_path = temp.path().join(CONTENT_MANIFEST_FILE_NAME);
        let mut contents = fs::read_to_string(&contents_path).unwrap();
        let first = contents.lines().next().unwrap().to_string();
        contents.push_str(&first);
        contents.push('\n');
        fs::write(contents_path, contents).unwrap();

        let error = verify_archive(temp.path()).unwrap_err();
        assert!(matches!(error, PackageError::DuplicateContentPath(_)));
    }

    #[test]
    fn invalid_hash_length_and_non_hex_are_rejected() {
        for invalid in [
            "abc",
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        ] {
            let (temp, _) = populated_archive();
            let contents_path = temp.path().join(CONTENT_MANIFEST_FILE_NAME);
            let contents = fs::read_to_string(&contents_path).unwrap();
            let (_, path) = contents.lines().next().unwrap().split_once("  ").unwrap();
            fs::write(&contents_path, format!("{invalid}  {path}\n")).unwrap();

            let error = verify_archive(temp.path()).unwrap_err();
            assert!(matches!(error, PackageError::InvalidContentHash { .. }));
        }
    }

    #[test]
    fn unlisted_ordinary_file_is_rejected() {
        let (temp, _) = populated_archive();
        fs::write(temp.path().join("surprise.txt"), "not declared").unwrap();

        let error = verify_archive(temp.path()).unwrap_err();
        assert!(matches!(error, PackageError::UnlistedFile(_)));
    }

    #[test]
    fn package_manifest_must_be_listed() {
        let (temp, _) = populated_archive();
        let contents_path = temp.path().join(CONTENT_MANIFEST_FILE_NAME);
        let contents = fs::read_to_string(&contents_path).unwrap();
        let filtered = contents
            .lines()
            .filter(|line| !line.ends_with(&format!("  {MANIFEST_FILE_NAME}")))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(contents_path, format!("{filtered}\n")).unwrap();

        let error = verify_archive(temp.path()).unwrap_err();
        assert!(matches!(error, PackageError::UnlistedFile(_)));
    }

    #[test]
    fn package_manifest_hash_is_verified_before_parsing() {
        let (temp, manifest) = populated_archive();
        let mut tampered = manifest;
        tampered.name = "Tampered but valid TOML".into();
        fs::write(
            temp.path().join(MANIFEST_FILE_NAME),
            tampered.to_toml_string().unwrap(),
        )
        .unwrap();

        let error = read_archive(temp.path()).unwrap_err();
        assert!(matches!(error, PackageError::HashMismatch { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_is_rejected_even_when_unlisted() {
        use std::os::unix::fs::symlink;

        let (temp, _) = populated_archive();
        symlink(MANIFEST_FILE_NAME, temp.path().join("manifest-link")).unwrap();

        let error = verify_archive(temp.path()).unwrap_err();
        assert!(matches!(error, PackageError::NotRegularFile(_)));
    }

    #[cfg(unix)]
    #[test]
    fn listed_file_swapped_for_symlink_is_rejected_via_opened_handle() {
        use std::os::unix::fs::symlink;

        // 先写入合法归档（SKILL.md 内容为 "# Search"），随后把该文件替换为指向
        // 归档外同内容文件的 symlink：读取必须经 O_NOFOLLOW 拒绝，而不是透过
        // 链接读入外部文件后按路径 lstat 的旧结果通过校验。
        let (temp, _) = populated_archive();
        let skill_file = temp.path().join("skills/search/SKILL.md");

        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("SKILL.md"), "# Search").unwrap();
        fs::remove_file(&skill_file).unwrap();
        symlink(outside.path().join("SKILL.md"), &skill_file).unwrap();

        let error = read_archive(temp.path()).unwrap_err();
        assert!(matches!(error, PackageError::NotRegularFile(_)));
    }

    #[cfg(unix)]
    #[test]
    fn special_file_is_rejected_even_when_unlisted() {
        use std::os::unix::net::UnixListener;

        let (temp, _) = populated_archive();
        let _socket = UnixListener::bind(temp.path().join("package.sock")).unwrap();

        let error = verify_archive(temp.path()).unwrap_err();
        assert!(matches!(error, PackageError::NotRegularFile(_)));
    }

    #[cfg(unix)]
    #[test]
    fn ancestor_directory_swapped_for_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        // 中间分量（`skills` 目录）被替换为指向归档外同构目录的 symlink：遍历
        // 必须经 root 句柄链的 openat(O_NOFOLLOW) 拒绝，而不是沿路径重新解析后
        // 透过链接读入外部目录。
        let (temp, _) = populated_archive();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir_all(outside.path().join("search")).unwrap();
        fs::write(outside.path().join("search/SKILL.md"), "# Search").unwrap();

        fs::remove_dir_all(temp.path().join("skills")).unwrap();
        symlink(outside.path(), temp.path().join("skills")).unwrap();

        let error = read_archive(temp.path()).unwrap_err();
        assert!(matches!(error, PackageError::NotRegularFile(_)));
    }

    #[cfg(unix)]
    #[test]
    fn read_file_rejects_post_verification_symlink_swap() {
        use std::os::unix::fs::symlink;

        // 归档验证完成后、冲突检测读取前，把子 manifest 替换为 symlink：
        // PackageArchive::read_file 经根句柄逐级 no-follow 打开，必须 fail-closed，
        // 而不是透过链接读入外部文件。
        let (temp, _) = populated_archive();
        let archive = read_archive(temp.path()).expect("read archive");
        let manifest_file = temp.path().join("skills/search/SKILL.md");

        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("SKILL.md"), "# Search").unwrap();
        fs::remove_file(&manifest_file).unwrap();
        symlink(outside.path().join("SKILL.md"), &manifest_file).unwrap();

        let error = archive
            .read_file(&PackageRelativePath::new("skills/search/SKILL.md").unwrap())
            .unwrap_err();
        assert!(matches!(error, PackageError::NotRegularFile(_)));
    }

    #[cfg(windows)]
    #[test]
    fn junction_is_rejected_at_every_level() {
        use std::os::windows::fs::symlink_dir;

        let (temp, _) = populated_archive();
        // junction 替换中间分量（`skills`）。
        let junction = temp.path().join("skills");
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir_all(outside.path().join("search")).unwrap();
        fs::write(outside.path().join("search/SKILL.md"), "# Search").unwrap();
        fs::remove_dir_all(&junction).unwrap();
        // 目录 junction 不要求管理员权限；失败（如策略限制）时跳过而非误报。
        if symlink_dir(outside.path(), &junction).is_err() {
            eprintln!("skipping junction test: symlink creation unavailable");
            return;
        }
        let error = read_archive(temp.path()).unwrap_err();
        assert!(matches!(error, PackageError::NotRegularFile(_)));
    }

    #[cfg(windows)]
    #[test]
    fn file_symlink_is_rejected_even_when_listed() {
        use std::os::windows::fs::symlink_file;

        let (temp, _) = populated_archive();
        let skill_file = temp.path().join("skills/search/SKILL.md");
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("SKILL.md"), "# Search").unwrap();
        fs::remove_file(&skill_file).unwrap();
        if symlink_file(outside.path().join("SKILL.md"), &skill_file).is_err() {
            eprintln!("skipping symlink test: symlink creation unavailable");
            return;
        }
        let error = read_archive(temp.path()).unwrap_err();
        assert!(matches!(error, PackageError::NotRegularFile(_)));
    }
}
