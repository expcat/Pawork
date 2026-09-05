//! Desktop 自有外观配置；不读写 Host config.toml（ADR-053）。
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopPreferences {
    pub language: String,
    pub text_scale: u16,
}

impl Default for DesktopPreferences {
    fn default() -> Self {
        Self {
            language: "en".into(),
            text_scale: 100,
        }
    }
}

fn config_path() -> io::Result<PathBuf> {
    let base = if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(|v| PathBuf::from(v).join("Library/Application Support/dev.pawork.pawork"))
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(|v| PathBuf::from(v).join("pawork/pawork/config"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|v| v.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|v| PathBuf::from(v).join(".config")))
            .map(|v| v.join("pawork"))
    };
    base.filter(|v| v.is_absolute())
        .map(|v| v.join("desktop.json"))
        .ok_or_else(|| io::Error::other("Desktop config directory unavailable"))
}

fn read(
    path: &Path,
) -> io::Result<(
    DesktopPreferences,
    serde_json::Map<String, serde_json::Value>,
)> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok((DesktopPreferences::default(), Default::default()));
        }
        Err(e) => return Err(e),
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    let table = value
        .as_object()
        .ok_or_else(|| io::Error::other("Desktop preferences must be an object"))?;
    let mut prefs = DesktopPreferences::default();
    if let Some(v) = table.get("language") {
        prefs.language = v
            .as_str()
            .filter(|v| matches!(*v, "en" | "zh"))
            .ok_or_else(|| io::Error::other("Invalid Desktop language"))?
            .into();
    }
    if let Some(v) = table.get("text_scale") {
        prefs.text_scale =
            v.as_u64()
                .filter(|v| matches!(v, 100 | 125 | 150))
                .ok_or_else(|| io::Error::other("Invalid Desktop text scale"))? as u16;
    }
    Ok((prefs, table.clone()))
}

static WRITE_LOCK: Mutex<()> = Mutex::new(());
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn write(path: &Path, prefs: &DesktopPreferences) -> io::Result<()> {
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (_, mut table) = read(path)?; // 损坏文件不覆盖；保留未知键。
    if !matches!(prefs.language.as_str(), "en" | "zh")
        || !matches!(prefs.text_scale, 100 | 125 | 150)
    {
        return Err(io::Error::other("Invalid Desktop preferences"));
    }
    table.insert("language".into(), prefs.language.clone().into());
    table.insert("text_scale".into(), prefs.text_scale.into());
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Missing config parent"))?;
    std::fs::create_dir_all(parent)?;
    let temp = path.with_extension(format!(
        "json.{}.{}.tmp",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = std::fs::write(
        &temp,
        serde_json::to_vec_pretty(&table).map_err(io::Error::other)?,
    )
    .and_then(|()| std::fs::rename(&temp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

pub fn load_preferences() -> Result<DesktopPreferences, String> {
    config_path()
        .and_then(|path| read(&path).map(|(prefs, _)| prefs))
        .map_err(|e| e.to_string())
}

pub fn save_preferences(prefs: &DesktopPreferences) -> Result<(), String> {
    config_path()
        .and_then(|path| write(&path, prefs))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_restore_and_preserve_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desktop.json");
        assert_eq!(read(&path).unwrap().0, DesktopPreferences::default());
        std::fs::write(&path, r#"{"future":true}"#).unwrap();
        let prefs = DesktopPreferences {
            language: "zh".into(),
            text_scale: 125,
        };
        write(&path, &prefs).unwrap();
        let (loaded, table) = read(&path).unwrap();
        assert_eq!(loaded, prefs);
        assert_eq!(table["future"], true);
    }

    #[test]
    fn damaged_preferences_are_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desktop.json");
        std::fs::write(&path, "{broken").unwrap();
        assert!(read(&path).is_err());
        assert!(write(&path, &DesktopPreferences::default()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{broken");
    }
}
