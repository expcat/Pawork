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
                .unwrap_or_else(|e| Err(e.to_string()));
            let _ = events
                .send(ControllerEvent::ComposerAttachmentsLoaded { draft, result })
                .await;
        });
    }
}

fn read_files(paths: Vec<PathBuf>, images_only: bool) -> Result<Vec<ComposerAttachment>, String> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    if paths.len() > 4 {
        return Err("Attach at most 4 files per message".into());
    }
    paths
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .ok_or("Missing file name")?
                .to_string_lossy()
                .into_owned();
            if !std::fs::metadata(&path).map_err(|e| format!("{name}: {e}"))?.is_file() {
                return Err(format!("{name}: select a regular file"));
            }
            let file = std::fs::File::open(&path).map_err(|e| format!("{name}: {e}"))?;
            if !file.metadata().map_err(|e| e.to_string())?.is_file() {
                return Err(format!("{name}: select a regular file"));
            }
            let mut bytes = Vec::new();
            file.take(8 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| format!("{name}: {e}"))?;
            if bytes.is_empty() || bytes.len() > 8 * 1024 * 1024 {
                return Err(format!("{name}: file must be between 1 byte and 8 MiB"));
            }
            let image = bytes.starts_with(b"\x89PNG\r\n\x1a\n")
                || bytes.starts_with(&[0xff, 0xd8, 0xff])
                || bytes.starts_with(b"GIF87a")
                || bytes.starts_with(b"GIF89a")
                || (bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"));
            if !image
                && (images_only
                    || bytes.len() > 64 * 1024
                    || bytes.contains(&0)
                    || std::str::from_utf8(&bytes).is_err())
            {
                return Err(format!(
                    "{name}: choose PNG, JPEG, GIF, WebP or UTF-8 text up to 64 KiB"
                ));
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
