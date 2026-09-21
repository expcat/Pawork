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
}

impl DesktopController {
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

#[cfg(test)]
mod tests {
    use super::*;

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
