//! GUI 客户端认证（P13-4）。
//!
//! 宿主把 [`TokenStore`] 指向的 token 文件交给 [`TokenAuthenticator`]，后者实现
//! [`gui-protocol::ClientAuthenticator`]：校验握手携带的
//! [`ClientAuthentication`]（scheme + proof）与 token 文件内容是否一致。
//!
//! 安全要点：
//! - token 在 [`Token`] 内不实现 `Serialize` / `Display`，`Debug` 输出脱敏；
//! - 比较使用 constant-time 算法，不因内容差异提前返回；
//! - Unix 上 token 文件与目录权限分别收紧为 0600 / 0700；
//! - 明文 token 不写入日志（协议日志必须执行 redaction，见 gui-protocol）。

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use gui_protocol::{ClientAuthentication, ClientAuthenticator, ProtocolError};
use rand::RngCore;
use thiserror::Error;

/// 握手凭证使用的认证 scheme。
pub const TOKEN_SCHEME: &str = "pawork-token";

/// 生成 token 的随机字节数（64 个十六进制字符）。
const TOKEN_BYTES: usize = 32;

/// 不透明 token：`Debug` 脱敏，不实现 `Serialize` / `Display`。
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// 以 constant-time 方式与候选字符串比较。
    pub fn constant_time_eq(&self, candidate: &str) -> bool {
        constant_time_eq(self.0.as_bytes(), candidate.as_bytes())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Token(\"***\")")
    }
}

#[derive(Debug, Error)]
pub enum ClientAuthError {
    #[error("token file already exists: {path}")]
    AlreadyExists { path: PathBuf },
    #[error("token file not found: {path}")]
    NotFound { path: PathBuf },
    #[error("token file is empty or malformed: {path}")]
    Malformed { path: PathBuf },
    #[error("failed to create token directory {path}: {source}")]
    CreateDir { path: PathBuf, source: io::Error },
    #[error("failed to write token file {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
    #[error("failed to read token file {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("failed to remove token file {path}: {source}")]
    Remove { path: PathBuf, source: io::Error },
    #[error("failed to set token permissions on {path}: {source}")]
    Permissions { path: PathBuf, source: io::Error },
}

/// token 文件的生成 / 加载 / 删除。
#[derive(Clone, Debug)]
pub struct TokenStore {
    path: PathBuf,
}

impl TokenStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 生成新 token 并写入文件；文件已存在时报错，绝不覆盖。
    pub fn generate(&self) -> Result<Token, ClientAuthError> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|source| ClientAuthError::CreateDir {
                    path: parent.to_path_buf(),
                    source,
                })?;
                #[cfg(unix)]
                set_mode(parent, 0o700)?;
            }
        }
        let mut bytes = [0u8; TOKEN_BYTES];
        rand::thread_rng().fill_bytes(&mut bytes);
        let token = Token(to_hex(&bytes));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.path)
            .map_err(|source| {
                if source.kind() == io::ErrorKind::AlreadyExists {
                    ClientAuthError::AlreadyExists {
                        path: self.path.clone(),
                    }
                } else {
                    ClientAuthError::Write {
                        path: self.path.clone(),
                        source,
                    }
                }
            })?;
        file.write_all(token.as_str().as_bytes())
            .and_then(|_| file.write_all(b"\n"))
            .and_then(|_| file.sync_all())
            .map_err(|source| ClientAuthError::Write {
                path: self.path.clone(),
                source,
            })?;
        #[cfg(unix)]
        set_mode(&self.path, 0o600)?;
        Ok(token)
    }

    /// 加载 token；文件缺失或内容为空 / 非 UTF-8 时返回对应错误。
    pub fn load(&self) -> Result<Token, ClientAuthError> {
        let bytes = fs::read(&self.path).map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                ClientAuthError::NotFound {
                    path: self.path.clone(),
                }
            } else {
                ClientAuthError::Read {
                    path: self.path.clone(),
                    source,
                }
            }
        })?;
        let text = String::from_utf8(bytes).map_err(|_| ClientAuthError::Malformed {
            path: self.path.clone(),
        })?;
        let token = text.trim();
        if token.is_empty() {
            return Err(ClientAuthError::Malformed {
                path: self.path.clone(),
            });
        }
        Ok(Token(token.to_string()))
    }

    pub fn delete(&self) -> Result<(), ClientAuthError> {
        fs::remove_file(&self.path).map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                ClientAuthError::NotFound {
                    path: self.path.clone(),
                }
            } else {
                ClientAuthError::Remove {
                    path: self.path.clone(),
                    source,
                }
            }
        })
    }
}

/// 握手认证器：校验 `ClientAuthentication` 与 token 文件内容。
#[derive(Clone, Debug)]
pub struct TokenAuthenticator {
    store: TokenStore,
}

impl TokenAuthenticator {
    pub fn new(store: TokenStore) -> Self {
        Self { store }
    }
}

impl ClientAuthenticator for TokenAuthenticator {
    fn authenticate(&self, authentication: &ClientAuthentication) -> Result<(), ProtocolError> {
        if authentication.scheme != TOKEN_SCHEME {
            return Err(ProtocolError::authentication_failed(format!(
                "unsupported authentication scheme {:?}",
                authentication.scheme
            )));
        }
        let token = match self.store.load() {
            Ok(token) => token,
            Err(ClientAuthError::NotFound { .. }) => {
                return Err(ProtocolError::authentication_failed(
                    "no token is configured on the server",
                ));
            }
            Err(error) => {
                return Err(ProtocolError::authentication_failed(format!(
                    "token store error: {error}"
                )));
            }
        };
        if token.constant_time_eq(&authentication.proof) {
            Ok(())
        } else {
            Err(ProtocolError::authentication_failed("invalid token"))
        }
    }
}

/// constant-time 比较：长度不同时直接失败（只泄露长度），逐字节异或累计。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left, right) in a.iter().zip(b) {
        difference |= left ^ right;
    }
    difference == 0
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), ClientAuthError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|source| {
        ClientAuthError::Permissions {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authentication(scheme: &str, proof: &str) -> ClientAuthentication {
        ClientAuthentication {
            scheme: scheme.into(),
            proof: proof.into(),
        }
    }

    #[test]
    fn generate_load_round_trip_and_debug_redaction() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = TokenStore::new(temp.path().join("gui.token"));
        let token = store.generate().expect("generate");
        assert_eq!(token.as_str().len(), TOKEN_BYTES * 2);
        assert!(!format!("{token:?}").contains(token.as_str()));
        assert_eq!(store.load().expect("load"), token);
        assert!(store.path().exists());
    }

    #[test]
    fn generate_never_overwrites_existing_token() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = TokenStore::new(temp.path().join("gui.token"));
        let first = store.generate().expect("generate");
        let error = store.generate().expect_err("must fail");
        assert!(matches!(error, ClientAuthError::AlreadyExists { .. }));
        assert_eq!(store.load().expect("load"), first);
    }

    #[test]
    fn generate_is_atomic_under_contention() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("gui.token");
        fs::write(&path, b"pre-existing content\n").expect("write");
        let store = TokenStore::new(&path);
        let error = store.generate().expect_err("must fail");
        assert!(matches!(error, ClientAuthError::AlreadyExists { .. }));
        assert_eq!(
            fs::read(&path).expect("read"),
            b"pre-existing content\n",
            "existing file must not be truncated"
        );
    }

    #[test]
    fn load_missing_file_reports_not_found() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = TokenStore::new(temp.path().join("missing.token"));
        assert!(matches!(
            store.load(),
            Err(ClientAuthError::NotFound { .. })
        ));
    }

    #[test]
    fn empty_token_file_is_malformed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = TokenStore::new(temp.path().join("gui.token"));
        fs::write(store.path(), b"  \n").expect("write");
        assert!(matches!(
            store.load(),
            Err(ClientAuthError::Malformed { .. })
        ));
    }

    #[test]
    fn constant_time_comparison() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"", b"a"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn authenticator_accepts_matching_token() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = TokenStore::new(temp.path().join("gui.token"));
        let token = store.generate().expect("generate");
        let authenticator = TokenAuthenticator::new(store);
        assert!(authenticator
            .authenticate(&authentication(TOKEN_SCHEME, token.as_str()))
            .is_ok());
    }

    #[test]
    fn authenticator_rejects_wrong_scheme_wrong_proof_and_missing_token() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = TokenStore::new(temp.path().join("gui.token"));
        let token = store.generate().expect("generate");
        let authenticator = TokenAuthenticator::new(store);

        let wrong_scheme = authenticator
            .authenticate(&authentication("other", token.as_str()))
            .expect_err("wrong scheme");
        assert_eq!(
            wrong_scheme.code,
            gui_protocol::ProtocolErrorCode::AuthenticationFailed
        );

        let wrong_proof = authenticator
            .authenticate(&authentication(TOKEN_SCHEME, "not-the-token"))
            .expect_err("wrong proof");
        assert_eq!(
            wrong_proof.code,
            gui_protocol::ProtocolErrorCode::AuthenticationFailed
        );

        let missing = TokenAuthenticator::new(TokenStore::new(temp.path().join("absent.token")));
        let missing_error = missing
            .authenticate(&authentication(TOKEN_SCHEME, token.as_str()))
            .expect_err("no token file");
        assert_eq!(
            missing_error.code,
            gui_protocol::ProtocolErrorCode::AuthenticationFailed
        );
    }

    #[test]
    fn delete_removes_token_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = TokenStore::new(temp.path().join("gui.token"));
        store.generate().expect("generate");
        store.delete().expect("delete");
        assert!(!store.path().exists());
        assert!(matches!(
            store.delete(),
            Err(ClientAuthError::NotFound { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn token_file_and_directory_permissions_are_restricted() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let token_dir = temp.path().join("tokens");
        let store = TokenStore::new(token_dir.join("gui.token"));
        store.generate().expect("generate");
        let file_mode = fs::metadata(store.path())
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        let dir_mode = fs::metadata(&token_dir)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
        assert_eq!(dir_mode, 0o700);
    }
}
