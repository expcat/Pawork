//! Save an explicitly reviewed recording in the existing skill layout.
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

/// Create a new workspace skill. Existing names are never replaced; manifest is written last.
pub fn write_skill_recording(
    roots: &[PathBuf],
    name: &str,
    description: &str,
    content: &str,
) -> io::Result<String> {
    if name.is_empty()
        || name.len() > 64
        || !name.as_bytes()[0].is_ascii_lowercase()
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid skill name",
        ));
    }
    let relative = format!(".pawork/skills/{name}");
    let resolved =
        pawork_policy::resolve_workspace_path(roots, &relative).map_err(io::Error::other)?;
    let mut manifest = toml::Table::new();
    manifest.insert("id".into(), name.into());
    manifest.insert("version".into(), "1.0.0".into());
    manifest.insert("description".into(), description.trim().into());
    let manifest = toml::to_string(&manifest).map_err(io::Error::other)?;
    write_new(&resolved.root, name, content, &manifest)?;
    Ok(format!("{relative}/SKILL.md"))
}

#[cfg(unix)]
fn write_new(root: &Path, name: &str, content: &str, manifest: &str) -> io::Result<()> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::OpenOptionsExt,
        },
    };
    fn directory(parent: &fs::File, name: &str, exclusive: bool) -> io::Result<fs::File> {
        let name = CString::new(name).map_err(io::Error::other)?;
        // Names are fixed components or validated identifiers; all traversal stays relative to held directory descriptors.
        if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } < 0 {
            let error = io::Error::last_os_error();
            if exclusive || error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error);
            }
        }
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat returned a fresh, owned descriptor.
        Ok(unsafe { fs::File::from_raw_fd(fd) })
    }
    let root = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)?;
    let resources = directory(&root, ".pawork", false)?;
    let skills = directory(&resources, "skills", false)?;
    let target = directory(&skills, name, true)?;
    for (name, body) in [("SKILL.md", content), ("manifest.toml", manifest)] {
        let name = CString::new(name).expect("static name");
        let fd = unsafe {
            libc::openat(
                target.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat returned a fresh, owned descriptor.
        let mut file = unsafe { fs::File::from_raw_fd(fd) };
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
    }
    target.sync_all()?;
    skills.sync_all()
}

#[cfg(not(unix))]
fn write_new(_: &Path, _: &str, _: &str, _: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Skill recording requires safe directory-relative writes, unavailable on this platform",
    ))
}
