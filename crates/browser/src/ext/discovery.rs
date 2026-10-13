//! Per-user discovery directory for live bridge processes. Each bridge
//! writes `<browser>-<pid>.json` (atomically, 0600) containing its socket
//! path and authentication token; Desktop scans the directory per action
//! and prunes entries it cannot connect to.

#![cfg(unix)]

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::BrowserKind;

pub fn bridge_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("external-browser")
}

#[derive(Clone, Debug)]
pub struct BridgeDiscovery {
    pub browser: BrowserKind,
    pub pid: u32,
    pub generation: String,
    pub socket: String,
    pub token: String,
}

impl BridgeDiscovery {
    pub fn socket_path(&self) -> PathBuf {
        PathBuf::from(&self.socket)
    }

    fn from_json(value: &Value) -> Option<Self> {
        let browser = BrowserKind::parse(value["browser"].as_str()?)?;
        Some(Self {
            browser,
            pid: value["pid"].as_u64()? as u32,
            generation: value["generation"].as_str()?.to_string(),
            socket: value["socket"].as_str()?.to_string(),
            token: value["token"].as_str()?.to_string(),
        })
    }
}

/// Create the discovery directory (0700) and write one entry atomically.
pub fn write_discovery(data_dir: &Path, entry: &BridgeDiscovery) -> io::Result<()> {
    let dir = bridge_dir(data_dir);
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    let payload = json!({
        "version": 1,
        "browser": entry.browser.name(),
        "pid": entry.pid,
        "generation": entry.generation,
        "socket": entry.socket,
        "token": entry.token,
        "started_ms": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0),
    });
    let body = serde_json::to_vec(&payload)?;
    let target = dir.join(format!("{}-{}.json", entry.browser.name(), entry.pid));
    let temp = dir.join(format!(
        ".{}.tmp",
        target.file_name().unwrap().to_string_lossy()
    ));
    fs::write(&temp, &body)?;
    fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))?;
    fs::rename(&temp, &target)?;
    Ok(())
}

/// Remove one bridge's discovery entry (best effort, used on shutdown).
pub fn remove_discovery(data_dir: &Path, entry: &BridgeDiscovery) {
    let path = bridge_dir(data_dir).join(format!("{}-{}.json", entry.browser.name(), entry.pid));
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(entry.socket_path());
}

/// Parse every readable discovery entry. Malformed files are skipped; the
/// caller proves liveness by connecting, so stale entries cost nothing.
pub fn scan_discoveries(data_dir: &Path) -> Vec<BridgeDiscovery> {
    let dir = bridge_dir(data_dir);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(body) = fs::read(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<Value>(&body) else {
            continue;
        };
        if let Some(discovery) = BridgeDiscovery::from_json(&value) {
            found.push(discovery);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_round_trip_and_scan_skips_malformed_files() {
        let dir = tempfile::tempdir().unwrap();
        let entry = BridgeDiscovery {
            browser: BrowserKind::Chrome,
            pid: std::process::id(),
            generation: "1-2-3".into(),
            socket: "/tmp/nowhere.sock".into(),
            token: "deadbeef".into(),
        };
        write_discovery(dir.path(), &entry).unwrap();
        let scanned = scan_discoveries(dir.path());
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].browser, BrowserKind::Chrome);
        assert_eq!(scanned[0].token, "deadbeef");
        assert_eq!(scanned[0].socket, "/tmp/nowhere.sock");
        let file_mode = fs::metadata(
            bridge_dir(dir.path()).join(format!("chrome-{}.json", std::process::id())),
        )
        .unwrap()
        .permissions()
        .mode();
        assert_eq!(file_mode & 0o777, 0o600);
        fs::write(bridge_dir(dir.path()).join("garbage.json"), "{not json").unwrap();
        fs::write(
            bridge_dir(dir.path()).join("unknown.json"),
            serde_json::to_string(&serde_json::json!({"browser": "netscape", "pid": 1, "generation": "g", "socket": "/s", "token": "t"})).unwrap(),
        )
        .unwrap();
        assert_eq!(scan_discoveries(dir.path()).len(), 1);
        remove_discovery(dir.path(), &entry);
        assert!(scan_discoveries(dir.path()).is_empty());
    }
}
