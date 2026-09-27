//! Per-client gateway credentials. Only token digests are persisted.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
};

#[derive(Clone)]
pub struct GatewayTokenStore {
    directory: PathBuf,
}

#[derive(Serialize, Deserialize)]
pub struct GatewayTokenInfo {
    pub id: String,
    pub client: String,
    pub created_at_ms: u64,
}

#[derive(Serialize, Deserialize)]
struct StoredToken {
    #[serde(flatten)]
    info: GatewayTokenInfo,
    digest: String,
}

pub fn valid_client(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
}
fn valid_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|c| c.is_ascii_hexdigit())
}
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "invalid gateway token identifier or client",
    )
}

impl GatewayTokenStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    /// The secret is returned once; list/authentication never return it.
    pub fn issue(&self, client: &str) -> io::Result<(GatewayTokenInfo, String)> {
        if !valid_client(client) {
            return Err(invalid());
        }
        fs::create_dir_all(&self.directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))?;
        }
        let mut random = [0u8; 48];
        getrandom::fill(&mut random)
            .map_err(|_| io::Error::other("gateway token entropy unavailable"))?;
        let hex: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let id = hex[..32].to_string();
        let secret = format!("pwg_{id}_{}", &hex[32..]);
        let stored = StoredToken {
            info: GatewayTokenInfo {
                id: id.clone(),
                client: client.into(),
                created_at_ms: crate::unix_millis(),
            },
            digest: blake3::hash(secret.as_bytes()).to_hex().to_string(),
        };
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(self.directory.join(format!("{id}.json")))?;
        file.write_all(&serde_json::to_vec(&stored)?)?;
        file.sync_all()?;
        Ok((stored.info, secret))
    }

    pub fn list(&self) -> io::Result<Vec<GatewayTokenInfo>> {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e),
        };
        let mut tokens = vec![];
        for entry in entries {
            let entry = entry?;
            if entry.path().extension().is_some_and(|e| e == "json") && entry.file_type()?.is_file()
            {
                let stored: StoredToken = serde_json::from_slice(&fs::read(entry.path())?)?;
                tokens.push(stored.info);
            }
        }
        tokens.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(tokens)
    }

    pub fn revoke(&self, id: &str) -> io::Result<()> {
        if !valid_id(id) {
            return Err(invalid());
        }
        fs::remove_file(self.directory.join(format!("{id}.json")))
    }

    pub fn authenticate(&self, bearer: &str) -> io::Result<Option<String>> {
        let Some((id, secret)) = bearer.strip_prefix("pwg_").and_then(|s| s.split_once('_')) else {
            return Ok(None);
        };
        if !valid_id(id) || secret.len() != 64 || !secret.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(None);
        }
        let bytes = match fs::read(self.directory.join(format!("{id}.json"))) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let stored: StoredToken = serde_json::from_slice(&bytes)?;
        let digest = blake3::Hash::from_hex(&stored.digest)
            .map_err(|_| io::Error::other("invalid gateway token digest"))?;
        // blake3::Hash equality uses constant-time comparison.
        Ok(
            (digest == blake3::hash(bearer.as_bytes()) && valid_client(&stored.info.client))
                .then_some(stored.info.client),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gateway_tokens_isolate_clients_and_revoke_without_persisting_secrets() {
        let temp = tempfile::tempdir().unwrap();
        let store = GatewayTokenStore::new(temp.path());
        let (a, token) = store.issue("momai").unwrap();
        let (_, other) = store.issue("other-editor").unwrap();
        assert_eq!(
            store.authenticate(&token).unwrap().as_deref(),
            Some("momai")
        );
        let bytes = fs::read_to_string(temp.path().join(format!("{}.json", a.id))).unwrap();
        assert!(!bytes.contains(&token));
        assert_eq!(store.list().unwrap().len(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(temp.path().join(format!("{}.json", a.id)))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert!(store.issue("../escape").is_err());
        assert!(store.revoke("../escape").is_err());
        assert!(store
            .authenticate("pwg_../escape_secret")
            .unwrap()
            .is_none());
        store.revoke(&a.id).unwrap();
        assert!(store.authenticate(&token).unwrap().is_none());
        assert_eq!(
            store.authenticate(&other).unwrap().as_deref(),
            Some("other-editor")
        );
    }
}
