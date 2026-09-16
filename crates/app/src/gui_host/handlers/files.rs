//! Explicit local GUI file operations. Never record file bodies in the command ledger.
use super::super::GuiHostAdapter;
use crate::gui_server::GuiHostError;
use pawork_domain::WorkspaceId;
use pawork_protocol::{
    AppCommand, AppCommandEnvelope, AppQuery, AppQueryEnvelope, AppResponse, CommandSource,
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

const MAX_TEXT: usize = 128 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);
// Serialize version check + replacement across GUI connections.
static SAVE_LOCK: Mutex<()> = Mutex::new(());
fn error(message: impl Into<String>) -> GuiHostError {
    GuiHostAdapter::host_error("file_error", message)
}
fn io(error: std::io::Error) -> GuiHostError {
    GuiHostAdapter::host_error("file_io", error.to_string())
}
fn authorize(source: &CommandSource, minor: u16) -> Result<(), GuiHostError> {
    if minor < 19 || !matches!(source, CommandSource::LocalGui { .. }) {
        return Err(error(
            "File operations require a local GUI connection with API 1.19",
        ));
    }
    Ok(())
}
async fn roots(adapter: &GuiHostAdapter, id: &WorkspaceId) -> Result<Vec<PathBuf>, GuiHostError> {
    Ok(adapter
        .core
        .read()
        .await
        .workspace_by_id(id)
        .map_err(GuiHostAdapter::app_error)?
        .roots)
}
fn protected(path: &Path) -> bool {
    path.components().any(|part| {
        let Component::Normal(name) = part else {
            return false;
        };
        let name = name.to_string_lossy().to_ascii_lowercase();
        matches!(
            name.as_str(),
            ".git" | ".ssh" | ".aws" | "auth.json" | "mcp-auth.json" | "gui.token" | ".env"
        ) || name.starts_with(".env.")
    })
}
fn resolve(roots: &[PathBuf], path: &str) -> Result<PathBuf, GuiHostError> {
    if protected(Path::new(path)) {
        return Err(error("Protected file is not available in the file editor"));
    }
    let resolved =
        pawork_policy::resolve_workspace_path(roots, path).map_err(|e| error(e.to_string()))?;
    // Do not follow even in-project links in the editor. Check each lexical component.
    let mut current = resolved.root.clone();
    for component in Path::new(path).components() {
        match component {
            Component::CurDir => continue,
            Component::Normal(name) => current.push(name),
            _ => {
                return Err(error(
                    "Only project-relative paths without parent traversal are supported",
                ));
            }
        }
        if fs::symlink_metadata(&current)
            .map_err(io)?
            .file_type()
            .is_symlink()
        {
            return Err(error("Symbolic links are not editable"));
        }
    }
    Ok(resolved.absolute)
}
fn read_text(path: &Path) -> Result<String, GuiHostError> {
    if !fs::symlink_metadata(path).map_err(io)?.is_file() {
        return Err(error("Only regular text files are supported"));
    }
    let file = fs::File::open(path).map_err(io)?;
    let metadata = file.metadata().map_err(io)?;
    if !metadata.is_file() || metadata.len() > MAX_TEXT as u64 {
        return Err(error("Only text files up to 128 KiB are supported"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_TEXT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() > MAX_TEXT || bytes.contains(&0) {
        return Err(error(
            "Only UTF-8 text files up to 128 KiB without NUL are supported",
        ));
    }
    String::from_utf8(bytes).map_err(|_| error("This file is not UTF-8 text"))
}
fn revision(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}
fn list(roots: &[PathBuf], path: &str) -> Result<(Vec<Value>, bool), GuiHostError> {
    let absolute = resolve(roots, path)?;
    let mut entries = Vec::new();
    let mut truncated = false;
    let mut budget = 0;
    for entry in fs::read_dir(absolute).map_err(io)? {
        let entry = entry.map_err(io)?;
        let kind = entry.file_type().map_err(io)?;
        if !kind.is_file() && !kind.is_dir() {
            continue;
        }
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let relative = if path == "." {
            name.clone()
        } else {
            format!("{path}/{name}")
        };
        if protected(Path::new(&relative))
            || relative
                .parse::<pawork_protocol::WorkspaceRelativePath>()
                .is_err()
        {
            continue;
        }
        let row = json!({"path":relative,"name":name,"is_dir":kind.is_dir()});
        budget += row.to_string().len();
        if entries.len() >= 1000 || budget > 512 * 1024 {
            truncated = true;
            break;
        }
        entries.push(row);
    }
    entries.sort_by(|a, b| {
        b["is_dir"]
            .as_bool()
            .cmp(&a["is_dir"].as_bool())
            .then_with(|| a["name"].as_str().cmp(&b["name"].as_str()))
    });
    Ok((entries, truncated))
}
fn save(
    roots: &[PathBuf],
    path: &str,
    content: &str,
    expected: &str,
) -> Result<String, GuiHostError> {
    if content.len() > MAX_TEXT || content.contains('\0') {
        return Err(error(
            "Only UTF-8 text up to 128 KiB without NUL is supported",
        ));
    }
    let _guard = SAVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let absolute = resolve(roots, path)?;
    let old = read_text(&absolute)?;
    let next_revision = revision(content);
    let current_revision = revision(&old);
    if current_revision != expected {
        // A retry after a lost receipt can acknowledge the bytes already on disk.
        if current_revision == next_revision {
            return Ok(next_revision);
        }
        return Err(error(
            "File changed on disk. Your edits are kept; reload before saving again.",
        ));
    }
    let permissions = fs::metadata(&absolute).map_err(io)?.permissions();
    if permissions.readonly() {
        return Err(error("This file is read-only"));
    }
    let parent = absolute
        .parent()
        .ok_or_else(|| error("Invalid file path"))?;
    let temp = parent.join(format!(
        ".pawork-edit-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp).map_err(io)?;
    let result = (|| {
        file.write_all(content.as_bytes()).map_err(io)?;
        file.set_permissions(permissions).map_err(io)?;
        file.sync_all().map_err(io)?;
        if resolve(roots, path)? != absolute || revision(&read_text(&absolute)?) != current_revision
        {
            return Err(error("File changed while saving. Your edits are kept."));
        }
        fs::rename(&temp, &absolute).map_err(io)?;
        Ok(next_revision)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub(crate) async fn query(
    adapter: &GuiHostAdapter,
    envelope: &AppQueryEnvelope,
) -> Result<AppResponse, GuiHostError> {
    authorize(&envelope.source, envelope.api_version.minor)?;
    let (workspace_id, path, directory) = match &envelope.query {
        AppQuery::WorkspaceFiles { workspace_id, path } => {
            (workspace_id, path.as_str().to_string(), true)
        }
        AppQuery::WorkspaceFileRead { workspace_id, path } => {
            (workspace_id, path.as_str().to_string(), false)
        }
        _ => unreachable!(),
    };
    let roots = roots(adapter, workspace_id).await?;
    let id = workspace_id.as_str().to_string();
    tokio::task::spawn_blocking(move || {
        let data = if directory {
            let (entries, truncated) = list(&roots, &path)?;
            json!({"workspace_id":id,"path":path,"entries":entries,"truncated":truncated})
        } else {
            let content = read_text(&resolve(&roots, &path)?)?;
            json!({"workspace_id":id,"path":path,"revision":revision(&content),"content":content})
        };
        Ok(AppResponse::Data(data))
    })
    .await
    .map_err(|_| error("File task failed"))?
}
pub(crate) async fn write(
    adapter: &GuiHostAdapter,
    envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    authorize(&envelope.source, envelope.api_version.minor)?;
    let AppCommand::WorkspaceFileWrite {
        workspace_id,
        path,
        content,
        expected_revision,
    } = command
    else {
        unreachable!()
    };
    let roots = roots(adapter, workspace_id).await?;
    let (id, path, content, expected) = (
        workspace_id.as_str().to_string(),
        path.as_str().to_string(),
        content.clone(),
        expected_revision.clone(),
    );
    tokio::task::spawn_blocking(move || {
        let revision = save(&roots, &path, &content, &expected)?;
        Ok(AppResponse::Data(
            json!({"workspace_id":id,"path":path,"revision":revision}),
        ))
    })
    .await
    .map_err(|_| error("File task failed"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_file_browse_edit_and_conflict() {
        let root = tempfile::tempdir().unwrap();
        let roots = vec![root.path().to_path_buf()];
        fs::create_dir(root.path().join("src")).unwrap();
        fs::write(root.path().join("notes.txt"), "hello\n").unwrap();
        let (entries, truncated) = list(&roots, ".").unwrap();
        assert!(!truncated);
        assert_eq!(entries[0]["name"], "src");
        let old = read_text(&resolve(&roots, "notes.txt").unwrap()).unwrap();
        let next = save(&roots, "notes.txt", "edited\n", &revision(&old)).unwrap();
        assert_eq!(
            fs::read_to_string(root.path().join("notes.txt")).unwrap(),
            "edited\n"
        );
        fs::write(root.path().join("notes.txt"), "external\n").unwrap();
        assert!(save(&roots, "notes.txt", "lost\n", &next).is_err());
        assert_eq!(
            fs::read_to_string(root.path().join("notes.txt")).unwrap(),
            "external\n"
        );
    }
    #[test]
    fn workspace_file_rejects_escape_protected_binary_and_symlinks() {
        let source = CommandSource::LocalGui {
            client_id: "file-test".into(),
        };
        assert!(authorize(&source, 19).is_ok());
        assert!(authorize(&source, 18).is_err());
        assert!(authorize(&CommandSource::Automation, 19).is_err());
        let root = tempfile::tempdir().unwrap();
        let roots = vec![root.path().to_path_buf()];
        for path in [
            "../outside",
            "/etc/passwd",
            ".git/config",
            ".env",
            "auth.json",
        ] {
            assert!(resolve(&roots, path).is_err(), "{path}");
        }
        fs::write(root.path().join("binary"), b"a\0b").unwrap();
        assert!(read_text(&root.path().join("binary")).is_err());
        fs::write(root.path().join("large"), vec![b'x'; MAX_TEXT + 1]).unwrap();
        assert!(read_text(&root.path().join("large")).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let read_only = root.path().join("readonly.txt");
            fs::write(&read_only, "kept").unwrap();
            fs::set_permissions(&read_only, fs::Permissions::from_mode(0o444)).unwrap();
            assert!(save(&roots, "readonly.txt", "changed", &revision("kept")).is_err());
            assert_eq!(fs::read_to_string(&read_only).unwrap(), "kept");
            std::os::unix::fs::symlink(root.path().join("binary"), root.path().join("link"))
                .unwrap();
            assert!(resolve(&roots, "link").is_err());
            assert!(save(&roots, "link", "replacement", "r1").is_err());
        }
    }
}
