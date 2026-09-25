//! 用户通过系统选择器选定的文件，在后台有界读取并经 GUI 协议上传。
use super::*;
use std::io::Read;

#[derive(Clone)]
pub struct ComposerAttachment {
    pub id: String,
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    pub image: bool,
}
impl std::fmt::Debug for ComposerAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComposerAttachment")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("byte_length", &self.bytes.len())
            .finish()
    }
}
#[derive(Clone, Debug, Default)]
pub struct ComposerOptions {
    pub attachments: Vec<ComposerAttachment>,
    pub web_search: Option<bool>,
    pub video_urls: Vec<pawork_client::VideoContent>,
    /// 最近一次读取失败（按会话保留；成功或重新选择时清除）。
    pub attachment_error: Option<ComposerAttachmentError>,
}

/// 单批读取失败的结构化原因：UI 据此映射本地化文案与下一步指引，
/// 不再透传英文自由文本（RV-03）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerAttachmentError {
    /// 一次选择（或叠加后）超过每条消息 4 个附件的上限。
    TooMany { count: usize },
    /// 所选路径没有文件名（根路径 / 结尾为 ..）。
    MissingName,
    /// 不是常规文件（目录等）。
    NotFile { name: String },
    /// 文件为空。
    Empty { name: String },
    /// 超过上限：image=true 适用 8 MiB 图片上限，否则 64 KiB 文本上限。
    TooLarge { name: String, image: bool },
    /// images_only=true 只描述图片格式；否则提示图片或 UTF-8 文本。
    UnsupportedType { name: String, images_only: bool },
    /// 打开或读取失败。
    Io { name: String },
    /// Folder snapshots are bounded to 128 files, 512 entries, depth 8 and 64 KiB.
    FolderLimit { name: String },
    Browser {
        error: pawork_browser::ExternalPageError,
    },
}

impl DesktopController {
    pub fn supports_folder_snapshots() -> bool {
        cfg!(any(
            target_os = "macos",
            all(
                target_os = "linux",
                any(target_arch = "x86_64", target_arch = "aarch64")
            )
        ))
    }

    pub fn load_browser_attachment(
        &self,
        draft: Option<String>,
        browser: pawork_browser::ExternalBrowser,
    ) {
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                let snapshot = pawork_browser::capture_external_page(browser)
                    .map_err(|error| ComposerAttachmentError::Browser { error })?;
                static NEXT: AtomicU64 = AtomicU64::new(1);
                Ok(vec![ComposerAttachment {
                    id: format!(
                        "browser-{}-{}",
                        std::process::id(),
                        NEXT.fetch_add(1, Ordering::Relaxed)
                    ),
                    name: format!("{}-page.txt", browser.name()),
                    bytes: Arc::new(snapshot.into_bytes()),
                    image: false,
                }])
            })
            .await
            .unwrap_or_else(|_| {
                Err(ComposerAttachmentError::Browser {
                    error: pawork_browser::ExternalPageError::Other(
                        "Browser snapshot task failed".into(),
                    ),
                })
            });
            let _ = events
                .send(ControllerEvent::ComposerAttachmentsLoaded { draft, result })
                .await;
        });
    }

    pub fn load_composer_attachments(
        &self,
        draft: Option<String>,
        paths: Vec<PathBuf>,
        images_only: bool,
    ) {
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let result = tokio::task::spawn_blocking(move || read_files(paths, images_only))
                .await
                .unwrap_or_else(|e| {
                    Err(ComposerAttachmentError::Io {
                        name: format!("background read: {e}"),
                    })
                });
            let _ = events
                .send(ControllerEvent::ComposerAttachmentsLoaded { draft, result })
                .await;
        });
    }
}

fn read_files(
    paths: Vec<PathBuf>,
    images_only: bool,
) -> Result<Vec<ComposerAttachment>, ComposerAttachmentError> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    if paths.len() > 4 {
        return Err(ComposerAttachmentError::TooMany { count: paths.len() });
    }
    paths
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .ok_or(ComposerAttachmentError::MissingName)?
                .to_string_lossy()
                .into_owned();
            if !images_only
                && std::fs::symlink_metadata(&path)
                    .map_err(|_| ComposerAttachmentError::Io { name: name.clone() })?
                    .is_dir()
            {
                let bytes = read_folder(&path, &name)?;
                return Ok(ComposerAttachment {
                    id: format!(
                        "desktop-{}-{}",
                        std::process::id(),
                        NEXT_ID.fetch_add(1, Ordering::Relaxed)
                    ),
                    name: format!("{name}.folder.txt"),
                    bytes: Arc::new(bytes),
                    image: false,
                });
            }
            if !std::fs::metadata(&path)
                .map_err(|_| ComposerAttachmentError::Io { name: name.clone() })?
                .is_file()
            {
                return Err(ComposerAttachmentError::NotFile { name });
            }
            let file = std::fs::File::open(&path)
                .map_err(|_| ComposerAttachmentError::Io { name: name.clone() })?;
            if !file
                .metadata()
                .map_err(|_| ComposerAttachmentError::Io { name: name.clone() })?
                .is_file()
            {
                return Err(ComposerAttachmentError::NotFile { name });
            }
            let mut bytes = Vec::new();
            file.take(8 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| ComposerAttachmentError::Io { name: name.clone() })?;
            let image = bytes.starts_with(b"\x89PNG\r\n\x1a\n")
                || bytes.starts_with(&[0xff, 0xd8, 0xff])
                || bytes.starts_with(b"GIF87a")
                || bytes.starts_with(b"GIF89a")
                || (bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"));
            if bytes.is_empty() {
                return Err(ComposerAttachmentError::Empty { name });
            }
            if bytes.len() > 8 * 1024 * 1024 {
                return Err(ComposerAttachmentError::TooLarge { name, image });
            }
            if !image {
                if images_only {
                    return Err(ComposerAttachmentError::UnsupportedType {
                        name,
                        images_only: true,
                    });
                }
                if bytes.len() > 64 * 1024 {
                    return Err(ComposerAttachmentError::TooLarge { name, image: false });
                }
                if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
                    return Err(ComposerAttachmentError::UnsupportedType {
                        name,
                        images_only: false,
                    });
                }
            }
            Ok(ComposerAttachment {
                id: format!(
                    "desktop-{}-{}",
                    std::process::id(),
                    NEXT_ID.fetch_add(1, Ordering::Relaxed)
                ),
                name,
                bytes: Arc::new(bytes),
                image,
            })
        })
        .collect()
}

/// An explicit directory selection grants a read-only snapshot, not workspace trust.
/// Hidden files, symlinks and non-text files are omitted and counted in the preview.
fn read_folder(path: &std::path::Path, name: &str) -> Result<Vec<u8>, ComposerAttachmentError> {
    let fail = || ComposerAttachmentError::Io { name: name.into() };
    let limit = || ComposerAttachmentError::FolderLimit { name: name.into() };
    let root = path.canonicalize().map_err(|_| fail())?;
    let selected = open_snapshot_file(None, &root).map_err(|_| fail())?;
    let mut pending = vec![(PathBuf::new(), selected, 0)];
    let mut files = Vec::new();
    let mut scanned = 0;
    let mut skipped = 0;
    let mut total = 0;
    while let Some((relative, directory, depth)) = pending.pop() {
        if depth > 8 {
            return Err(limit());
        }
        let mut entries = Vec::new();
        // Names may change during enumeration; all actual reads stay relative to
        // the held directory, so a renamed/replaced ancestor cannot redirect them.
        for entry in std::fs::read_dir(root.join(&relative)).map_err(|_| fail())? {
            scanned += 1;
            if scanned > 512 {
                return Err(limit());
            }
            entries.push(entry.map_err(|_| fail())?);
        }
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let kind = entry.file_type().map_err(|_| fail())?;
            if entry.file_name().to_string_lossy().starts_with('.') || kind.is_symlink() {
                skipped += 1;
                continue;
            }
            if !kind.is_file() && !kind.is_dir() {
                skipped += 1;
                continue;
            }
            let child = relative.join(entry.file_name());
            let file =
                open_snapshot_file(Some(&directory), std::path::Path::new(&entry.file_name()))
                    .map_err(|_| fail())?;
            let metadata = file.metadata().map_err(|_| fail())?;
            if metadata.is_dir() {
                pending.push((child, file, depth + 1));
                continue;
            }
            if !metadata.is_file() {
                return Err(fail());
            }
            let mut bytes = Vec::new();
            file.take(64 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| fail())?;
            if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
                skipped += 1;
                continue;
            }
            total += bytes.len();
            if total > 64 * 1024 || files.len() >= 128 {
                return Err(limit());
            }
            files.push(serde_json::json!({
                "path": child.to_string_lossy(),
                "content": std::str::from_utf8(&bytes).map_err(|_| fail())?,
            }));
        }
    }
    files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    let bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "folder": name, "files": files, "omitted_entries": skipped,
        "note": "Read-only snapshot. Hidden files, symbolic links and non-text files omitted.",
    }))
    .map_err(|_| fail())?;
    if bytes.len() > 64 * 1024 {
        return Err(limit());
    }
    Ok(bytes)
}

/// Minimal native openat boundary; no runtime or dependency is added to Desktop.
/// ABI flags match Darwin and Linux libc headers for the supported desktop targets.
#[cfg(any(
    target_os = "macos",
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
))]
fn open_snapshot_file(
    parent: Option<&std::fs::File>,
    path: &std::path::Path,
) -> std::io::Result<std::fs::File> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    };
    #[cfg(target_os = "macos")]
    const FLAGS: i32 = 0x100 | 0x01000000 | 0x4; // NOFOLLOW | CLOEXEC | NONBLOCK
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    const FLAGS: i32 = 0x20000 | 0x80000 | 2048;
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    const FLAGS: i32 = 0x8000 | 0x80000 | 2048;
    let Some(parent) = parent else {
        return std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FLAGS)
            .open(path);
    };
    let component = path.as_os_str().as_bytes();
    if component.is_empty() || component.contains(&b'/') || component == b"." || component == b".."
    {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }
    let component = std::ffi::CString::new(component).map_err(std::io::Error::other)?;
    unsafe extern "C" {
        fn openat(
            fd: std::ffi::c_int,
            path: *const std::ffi::c_char,
            flags: std::ffi::c_int,
        ) -> std::ffi::c_int;
    }
    // SAFETY: borrowed live directory fd, NUL-terminated single component; no create flag.
    let fd = unsafe { openat(parent.as_raw_fd(), component.as_ptr(), FLAGS) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: openat returned a fresh owned descriptor.
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

#[cfg(not(any(
    target_os = "macos",
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
)))]
fn open_snapshot_file(
    _: Option<&std::fs::File>,
    _: &std::path::Path,
) -> std::io::Result<std::fs::File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Safe folder snapshots are unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_snapshot_preserves_paths_and_reports_omitted_entries() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        std::fs::write(dir.path().join("nested/readme.txt"), "hello").unwrap();
        std::fs::write(dir.path().join(".env"), "not attached").unwrap();
        std::fs::write(dir.path().join("binary"), [0, 1]).unwrap();
        let attachments = read_files(vec![dir.path().to_path_buf()], false).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&attachments[0].bytes).unwrap();
        assert_eq!(
            value["files"],
            serde_json::json!([{"path":"nested/readme.txt", "content":"hello"}])
        );
        assert_eq!(value["omitted_entries"], 2);
        std::fs::write(dir.path().join("large.txt"), vec![b'x'; 65537]).unwrap();
        assert!(matches!(
            read_folder(dir.path(), "folder"),
            Err(ComposerAttachmentError::FolderLimit { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn folder_snapshot_does_not_follow_symlinks_outside_selection() {
        let selected = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("private.txt"), "outside").unwrap();
        std::os::unix::fs::symlink(outside.path(), selected.path().join("escape")).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&read_folder(selected.path(), "folder").unwrap()).unwrap();
        assert!(value["files"].as_array().unwrap().is_empty());
        assert_eq!(value["omitted_entries"], 1);
        let held = open_snapshot_file(None, selected.path()).unwrap();
        std::fs::write(selected.path().join("kept.txt"), "inside").unwrap();
        assert!(open_snapshot_file(Some(&held), std::path::Path::new("escape")).is_err());
        // Swapping the visible directory cannot redirect the held directory read.
        let moved = outside.path().join("moved");
        std::fs::rename(selected.path(), &moved).unwrap();
        std::os::unix::fs::symlink(outside.path(), selected.path()).unwrap();
        let mut text = String::new();
        open_snapshot_file(Some(&held), std::path::Path::new("kept.txt"))
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "inside");
        std::fs::remove_file(selected.path()).unwrap();
        std::fs::rename(moved, selected.path()).unwrap();
    }

    fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
        // 独立目录承载，保留真实文件名（read_files 错误按 file_name 回报，
        // 断言依赖精确文件名）。
        let dir = std::env::temp_dir().join(format!("pawork-attach-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// RV-03 主路径：结构化错误按类型可分辨，images_only 拒绝不再与文本
    /// 规则混在同一条文案里；图片魔数仍走图片管线。
    #[test]
    fn read_files_maps_structured_failures() {
        let text = temp_file("a.txt", "hello".as_bytes());
        assert_eq!(
            read_files(vec![text.clone()], true).unwrap_err(),
            ComposerAttachmentError::UnsupportedType {
                name: "a.txt".into(),
                images_only: true,
            }
        );
        let loaded = read_files(vec![text.clone()], false).unwrap();
        assert_eq!(loaded[0].name, "a.txt");
        assert!(!loaded[0].image);

        let big_text = temp_file("big.txt", &vec![b'a'; 65 * 1024]);
        assert_eq!(
            read_files(vec![big_text], false).unwrap_err(),
            ComposerAttachmentError::TooLarge {
                name: "big.txt".into(),
                image: false,
            }
        );

        let png = temp_file("p.png", b"\x89PNG\r\n\x1a\nbody");
        let loaded = read_files(vec![png], true).unwrap();
        assert!(loaded[0].image);

        let too_many = (0..5)
            .map(|ix| temp_file(&format!("n{ix}"), b"x"))
            .collect::<Vec<_>>();
        assert_eq!(
            read_files(too_many.clone(), false).unwrap_err(),
            ComposerAttachmentError::TooMany { count: 5 }
        );
        for path in std::iter::once(text).chain(too_many) {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub(super) async fn upload(
    client: &GuiClient,
    session: &str,
    attachments: &[ComposerAttachment],
) -> Result<(), String> {
    for attachment in attachments {
        for (index, chunk) in attachment.bytes.chunks(64 * 1024).enumerate() {
            let command = serde_json::from_value(json!({ "method": "attachment_upload", "params": {
                "session_id": session, "attachment_id": attachment.id, "name": attachment.name,
                "offset": index * 64 * 1024, "total_bytes": attachment.bytes.len(), "data": chunk
            }})).map_err(|e| e.to_string())?;
            let response = client
                .command(command, command_source(), actor_identity())
                .await
                .map_err(|e| e.to_string())?;
            if !matches!(
                response.response,
                AppResponse::Data(_) | AppResponse::Accepted { .. }
            ) {
                return Err(format!(
                    "Attachment upload rejected: {:?}",
                    response.response
                ));
            }
        }
    }
    Ok(())
}
