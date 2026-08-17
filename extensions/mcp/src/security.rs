//! MCP secret references.
//!
//! MCP servers are configured with secrets (env vars for stdio servers, auth
//! headers for http servers). Configuration only ever persists a [`SecretRef`] —
//! the `(service, account)` locator into a [`SecretBackend`]. Plaintext is
//! resolved lazily at transport construction time and never enters serialized
//! state, logs, or error messages.

use std::fmt;

use pawork_auth::{AuthError, SecretBackend};
use serde::{Deserialize, Serialize};

use crate::McpError;

/// Locator for a plaintext secret held by a [`SecretBackend`].
///
/// Only `service` and `account` are persisted/serialized — they are keychain
/// locators, never the secret itself. `Debug` / `Serialize` / `Display`
/// therefore cannot leak plaintext by construction.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SecretRef {
    service: String,
    account: String,
}

impl SecretRef {
    /// Create a new secret reference from its backend locators.
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
        }
    }

    /// Backend `service` (keychain namespace) used to locate the secret.
    pub fn service(&self) -> &str {
        &self.service
    }

    /// Backend `account` used to locate the secret.
    pub fn account(&self) -> &str {
        &self.account
    }

    /// Resolve the plaintext secret from `backend`.
    ///
    /// The returned [`ResolvedSecret`] redacts its `Debug` / `Display`; callers
    /// must not persist or log it. Resolution failures are mapped to
    /// [`McpError::Secret`] without ever embedding plaintext (which never
    /// leaves the backend).
    pub fn resolve(&self, backend: &dyn SecretBackend) -> Result<ResolvedSecret, McpError> {
        let secret = backend
            .get(&self.service, &self.account)
            .map_err(map_auth_error)?;
        Ok(ResolvedSecret::new(secret))
    }
}

/// A resolved plaintext secret.
///
/// `Debug` / `Display` are redacted so the value cannot be accidentally logged.
/// Drop clears the logical contents before release; Rust's `String` does not
/// guarantee physical-memory zeroization.
pub struct ResolvedSecret {
    secret: String,
}

impl ResolvedSecret {
    pub(crate) fn new(secret: String) -> Self {
        Self { secret }
    }

    /// Expose the plaintext to the transport adapter that constructs the actual
    /// authentication header / env var. Must never be logged or persisted.
    pub fn expose_secret(&self) -> &str {
        &self.secret
    }
}

impl fmt::Debug for ResolvedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ResolvedSecret([REDACTED])")
    }
}

impl fmt::Display for ResolvedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl Drop for ResolvedSecret {
    fn drop(&mut self) {
        self.secret.clear();
    }
}

fn map_auth_error(error: AuthError) -> McpError {
    match error {
        AuthError::NotFound => McpError::Secret("secret not found in backend".into()),
        other => McpError::Secret(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_auth::MemoryBackend;

    const SECRET: &str = "sk-mcp-supersecret-abcdef";

    #[test]
    fn secret_ref_serializes_only_locators() {
        let reference = SecretRef::new("pawork.mcp.filesystem", "cred-1");
        let json = serde_json::to_value(&reference).expect("serialize");
        assert_eq!(json["service"], "pawork.mcp.filesystem");
        assert_eq!(json["account"], "cred-1");
        assert!(json.get("secret").is_none());
        assert!(json.to_string().find(SECRET).is_none());

        let back: SecretRef = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, reference);
    }

    #[test]
    fn debug_output_contains_no_plaintext() {
        let reference = SecretRef::new("pawork.mcp", "cred-1");
        assert!(!format!("{reference:?}").contains(SECRET));
    }

    #[test]
    fn resolve_returns_plaintext_without_leaking_in_debug() {
        let backend = MemoryBackend::new();
        backend
            .store("pawork.mcp", "cred-1", SECRET)
            .expect("store");

        let resolved = SecretRef::new("pawork.mcp", "cred-1")
            .resolve(&backend)
            .expect("resolve");
        assert_eq!(resolved.expose_secret(), SECRET);
        assert!(!format!("{resolved:?}").contains(SECRET));
        assert_eq!(format!("{resolved}"), "[REDACTED]");
    }

    #[test]
    fn resolve_missing_maps_to_secret_error_without_plaintext() {
        let backend = MemoryBackend::new();
        let error = SecretRef::new("pawork.mcp", "missing")
            .resolve(&backend)
            .expect_err("missing secret should fail");
        match error {
            McpError::Secret(message) => {
                assert!(message.contains("not found"));
                assert!(!message.contains(SECRET));
            }
            other => panic!("expected Secret error, got {other:?}"),
        }
    }
}
