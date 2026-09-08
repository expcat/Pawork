//! Provider account metadata and selection. Secrets and index commit together.
use pawork_domain::{CredentialId, ProviderId, Timestamp};
use serde::{Deserialize, Serialize};

use crate::default_credential::{delete_oauth_at, load_oauth_at, now_unix_millis, store_oauth_at};
use crate::locator::secret_service_for;
use crate::{AuthError, MaskedCredential, SecretBackend, StoredCredential, TokenSet};

pub const LEGACY_API_KEY_ID: &str = "default-api-key";
pub const LEGACY_OAUTH_ID: &str = "default-oauth";
const INDEX_ACCOUNT: &str = "accounts.meta";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAccountKind {
    ApiKey,
    #[serde(rename = "oauth")]
    OAuth,
}

impl ProviderAccountKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::OAuth => "oauth",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AccountEntry {
    credential_id: String,
    kind: ProviderAccountKind,
    display_name: String,
    created_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AccountIndex {
    version: u32,
    #[serde(default)]
    revision: u64,
    accounts: Vec<AccountEntry>,
    selected_credential_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ProviderAccount {
    pub credential_id: String,
    pub kind: ProviderAccountKind,
    pub display_name: String,
    pub created_at_ms: Option<u64>,
    pub stored: StoredCredential,
}

#[derive(Clone, Debug)]
pub struct ProviderAccounts {
    pub accounts: Vec<ProviderAccount>,
    pub selected_credential_id: Option<String>,
}

impl ProviderAccounts {
    pub fn selected(&self) -> Option<&ProviderAccount> {
        self.selected_credential_id
            .as_ref()
            .and_then(|id| self.accounts.iter().find(|a| &a.credential_id == id))
    }
}

fn optional(
    backend: &dyn SecretBackend,
    service: &str,
    account: &str,
) -> Result<Option<String>, AuthError> {
    match backend.get(service, account) {
        Ok(value) => Ok(Some(value)),
        Err(AuthError::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
}

fn slot(entry: &AccountEntry) -> &str {
    match entry.credential_id.as_str() {
        LEGACY_API_KEY_ID | LEGACY_OAUTH_ID => "default",
        id => id,
    }
}

fn malformed() -> AuthError {
    AuthError::MalformedMetadata("invalid provider account index".into())
}

fn read_index(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
) -> Result<AccountIndex, AuthError> {
    let service = secret_service_for(provider);
    if let Some(value) = optional(backend, &service, INDEX_ACCOUNT)? {
        // Never include serde's offending input in an authentication error.
        let index: AccountIndex = serde_json::from_str(&value).map_err(|_| malformed())?;
        let mut ids = std::collections::HashSet::new();
        if index.version != 1
            || index.accounts.iter().any(|entry| {
                let id = entry.credential_id.as_str();
                let valid = match id {
                    LEGACY_API_KEY_ID => entry.kind == ProviderAccountKind::ApiKey,
                    LEGACY_OAUTH_ID => entry.kind == ProviderAccountKind::OAuth,
                    _ => {
                        id.starts_with("cred_")
                            && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                    }
                };
                !valid || entry.display_name.trim().is_empty() || !ids.insert(id)
            })
            || index
                .selected_credential_id
                .as_ref()
                .is_some_and(|id| !ids.contains(id.as_str()))
        {
            return Err(malformed());
        }
        return Ok(index);
    }
    let mut accounts = Vec::new();
    if optional(backend, &service, "default")?.is_some() {
        accounts.push(AccountEntry {
            credential_id: LEGACY_API_KEY_ID.into(),
            kind: ProviderAccountKind::ApiKey,
            display_name: "Default API key".into(),
            created_at_ms: None,
        });
    }
    if let Some(stored) = load_oauth_at(backend, provider, "default")? {
        accounts.push(AccountEntry {
            credential_id: LEGACY_OAUTH_ID.into(),
            kind: ProviderAccountKind::OAuth,
            display_name: "Default OAuth".into(),
            created_at_ms: Some(stored.created_at.as_unix_millis()),
        });
    }
    Ok(AccountIndex {
        version: 1,
        revision: 0,
        accounts,
        selected_credential_id: None,
    })
}

fn stored_key(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
    entry: &AccountEntry,
) -> Result<StoredCredential, AuthError> {
    let service = secret_service_for(provider);
    let account = slot(entry);
    let secret = backend.get(&service, account)?;
    if secret.is_empty() {
        return Err(AuthError::InvalidSecret("stored API key is empty".into()));
    }
    let mut stored = StoredCredential::new(
        CredentialId::new(account),
        provider.clone(),
        entry.display_name.clone(),
        MaskedCredential::mask(&secret),
        service,
        account,
        Vec::new(),
    );
    stored.created_at = Timestamp::from_unix_millis(entry.created_at_ms.unwrap_or(0));
    Ok(stored)
}

fn load_account(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
    entry: &AccountEntry,
) -> Result<ProviderAccount, AuthError> {
    let mut stored = match entry.kind {
        ProviderAccountKind::ApiKey => stored_key(backend, provider, entry)?,
        ProviderAccountKind::OAuth => {
            let stored =
                load_oauth_at(backend, provider, slot(entry))?.ok_or(AuthError::NotFound)?;
            if backend
                .get(&stored.secret_service, &stored.secret_account)?
                .is_empty()
            {
                return Err(AuthError::InvalidSecret(
                    "stored OAuth token is empty".into(),
                ));
            }
            stored
        }
    };
    stored.display_name = entry.display_name.clone();
    Ok(ProviderAccount {
        credential_id: entry.credential_id.clone(),
        kind: entry.kind,
        display_name: entry.display_name.clone(),
        created_at_ms: entry.created_at_ms,
        stored,
    })
}

pub fn list_provider_accounts(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
) -> Result<ProviderAccounts, AuthError> {
    let mut result = None;
    backend.transaction(&mut |snapshot| {
        let index = read_index(snapshot, provider)?;
        result = Some(ProviderAccounts {
            accounts: index
                .accounts
                .iter()
                .map(|entry| load_account(snapshot, provider, entry))
                .collect::<Result<_, _>>()?,
            selected_credential_id: index.selected_credential_id,
        });
        Ok(())
    })?;
    result.ok_or_else(malformed)
}

pub fn provider_accounts_revision(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
) -> Result<Option<u64>, AuthError> {
    if optional(backend, &secret_service_for(provider), INDEX_ACCOUNT)?.is_none() {
        return Ok(None);
    }
    Ok(Some(read_index(backend, provider)?.revision))
}

fn edit<T>(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
    mut operation: impl FnMut(&dyn SecretBackend, &mut AccountIndex) -> Result<T, AuthError>,
) -> Result<T, AuthError> {
    let mut result = None;
    backend.transaction(&mut |transaction| {
        let mut index = read_index(transaction, provider)?;
        result = Some(operation(transaction, &mut index)?);
        index.revision = index.revision.checked_add(1).ok_or_else(malformed)?;
        let value = serde_json::to_string(&index).map_err(|_| malformed())?;
        transaction.store(&secret_service_for(provider), INDEX_ACCOUNT, &value)
    })?;
    result.ok_or_else(malformed)
}

pub fn validate_account_name(name: &str) -> Result<&str, AuthError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AuthError::InvalidSecret("account name is empty".into()));
    }
    Ok(name)
}

fn new_entry(name: &str, kind: ProviderAccountKind) -> Result<AccountEntry, AuthError> {
    Ok(AccountEntry {
        credential_id: crate::credential::generate_credential_id().as_str().into(),
        kind,
        display_name: validate_account_name(name)?.into(),
        created_at_ms: Some(now_unix_millis()),
    })
}

pub fn add_api_key_account(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
    name: &str,
    secret: &str,
    activate_if_empty: bool,
) -> Result<ProviderAccount, AuthError> {
    if secret.is_empty() {
        return Err(AuthError::InvalidSecret("secret is empty".into()));
    }
    let entry = new_entry(name, ProviderAccountKind::ApiKey)?;
    edit(backend, provider, |transaction, index| {
        transaction.store(&secret_service_for(provider), slot(&entry), secret)?;
        if index.accounts.is_empty() && activate_if_empty {
            index.selected_credential_id = Some(entry.credential_id.clone());
        }
        index.accounts.push(entry.clone());
        load_account(transaction, provider, &entry)
    })
}

pub fn add_oauth_account(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
    name: &str,
    tokens: &TokenSet,
    activate_if_empty: bool,
) -> Result<ProviderAccount, AuthError> {
    let entry = new_entry(name, ProviderAccountKind::OAuth)?;
    edit(backend, provider, |transaction, index| {
        store_oauth_at(transaction, provider.clone(), slot(&entry), tokens)?;
        if index.accounts.is_empty() && activate_if_empty {
            index.selected_credential_id = Some(entry.credential_id.clone());
        }
        index.accounts.push(entry.clone());
        load_account(transaction, provider, &entry)
    })
}

pub fn select_provider_account(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
    id: &str,
) -> Result<(), AuthError> {
    edit(backend, provider, |transaction, index| {
        let entry = index
            .accounts
            .iter()
            .find(|entry| entry.credential_id == id)
            .ok_or(AuthError::NotFound)?;
        load_account(transaction, provider, entry)?;
        index.selected_credential_id = Some(id.into());
        Ok(())
    })
}

fn delete_entry(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
    entry: &AccountEntry,
) -> Result<(), AuthError> {
    match entry.kind {
        ProviderAccountKind::ApiKey => {
            match backend.delete(&secret_service_for(provider), slot(entry)) {
                Ok(()) | Err(AuthError::NotFound) => Ok(()),
                Err(error) => Err(error),
            }
        }
        ProviderAccountKind::OAuth => delete_oauth_at(backend, provider, slot(entry)),
    }
}

pub fn remove_provider_account(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
    id: &str,
    effective_id: Option<&str>,
) -> Result<(), AuthError> {
    edit(backend, provider, |transaction, index| {
        // When selection is implicit, the Host supplies the actual channel-selected ID.
        let active = index.selected_credential_id.as_deref().or(effective_id);
        if active == Some(id) && index.accounts.len() > 1 {
            return Err(AuthError::InvalidSecret(
                "select another account before removing this account".into(),
            ));
        }
        let entry = index
            .accounts
            .iter()
            .find(|entry| entry.credential_id == id)
            .ok_or(AuthError::NotFound)?;
        delete_entry(transaction, provider, entry)?;
        index.accounts.retain(|entry| entry.credential_id != id);
        if index.selected_credential_id.as_deref() == Some(id) {
            index.selected_credential_id = None;
        }
        Ok(())
    })
}

pub fn remove_all_provider_accounts(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
) -> Result<(), AuthError> {
    edit(backend, provider, |transaction, index| {
        for entry in &index.accounts {
            delete_entry(transaction, provider, entry)?;
        }
        // Also clean orphan legacy OAuth token slots, matching existing logout semantics.
        delete_oauth_at(transaction, provider, "default")?;
        index.accounts.clear();
        index.selected_credential_id = None;
        Ok(())
    })
}

pub(crate) fn remove_legacy(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
    id: &str,
) -> Result<(), AuthError> {
    edit(backend, provider, |transaction, index| {
        let kind = if id == LEGACY_API_KEY_ID {
            ProviderAccountKind::ApiKey
        } else {
            ProviderAccountKind::OAuth
        };
        delete_entry(
            transaction,
            provider,
            &AccountEntry {
                credential_id: id.into(),
                kind,
                display_name: "default".into(),
                created_at_ms: None,
            },
        )?;
        index.accounts.retain(|entry| entry.credential_id != id);
        if index.selected_credential_id.as_deref() == Some(id) {
            index.selected_credential_id = None;
        }
        Ok(())
    })
}

pub(crate) fn store_legacy_api_key(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
    secret: &str,
) -> Result<StoredCredential, AuthError> {
    if secret.is_empty() {
        return Err(AuthError::InvalidSecret("secret is empty".into()));
    }
    edit(backend, provider, |transaction, index| {
        transaction.store(&secret_service_for(provider), "default", secret)?;
        let entry = AccountEntry {
            credential_id: LEGACY_API_KEY_ID.into(),
            kind: ProviderAccountKind::ApiKey,
            display_name: "Default API key".into(),
            created_at_ms: None,
        };
        if !index
            .accounts
            .iter()
            .any(|e| e.credential_id == LEGACY_API_KEY_ID)
        {
            index.accounts.insert(0, entry.clone());
        }
        stored_key(transaction, provider, &entry)
    })
}

pub(crate) fn store_legacy_oauth(
    backend: &dyn SecretBackend,
    provider: ProviderId,
    tokens: &TokenSet,
) -> Result<StoredCredential, AuthError> {
    edit(backend, &provider, |transaction, index| {
        let stored = store_oauth_at(transaction, provider.clone(), "default", tokens)?;
        if !index
            .accounts
            .iter()
            .any(|e| e.credential_id == LEGACY_OAUTH_ID)
        {
            index.accounts.push(AccountEntry {
                credential_id: LEGACY_OAUTH_ID.into(),
                kind: ProviderAccountKind::OAuth,
                display_name: "Default OAuth".into(),
                created_at_ms: Some(stored.created_at.as_unix_millis()),
            });
        }
        Ok(stored)
    })
}

pub(crate) fn refresh_account_tokens(
    backend: &dyn SecretBackend,
    stored: &mut StoredCredential,
    tokens: &TokenSet,
) -> Result<(), AuthError> {
    let provider = stored.provider.clone();
    let id = if stored.id.as_str() == "default" {
        LEGACY_OAUTH_ID
    } else {
        stored.id.as_str()
    }
    .to_string();
    edit(backend, &provider, |transaction, index| {
        if !index.accounts.iter().any(|account| {
            account.credential_id == id && account.kind == ProviderAccountKind::OAuth
        }) {
            return Err(AuthError::NotFound);
        }
        crate::default_credential::update_oauth_at(transaction, stored, tokens)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileBackend, MemoryBackend};

    fn tokens(access: &str) -> TokenSet {
        TokenSet {
            access_token: access.into(),
            refresh_token: Some(format!("{access}-refresh")),
            id_token: None,
            expires_in: Some(0),
            token_type: "Bearer".into(),
            scope: None,
        }
    }

    #[test]
    fn accounts_preserve_legacy_selection_and_atomic_inventory() {
        let provider = ProviderId::new("xai");
        let memory = MemoryBackend::new();
        memory
            .store(&secret_service_for(&provider), "default", "legacy-key")
            .unwrap();
        assert_eq!(
            list_provider_accounts(&memory, &provider).unwrap().accounts[0].credential_id,
            LEGACY_API_KEY_ID
        );
        assert!(matches!(
            memory.get(&secret_service_for(&provider), INDEX_ACCOUNT),
            Err(AuthError::NotFound)
        ));
        let added = add_api_key_account(&memory, &provider, "Work", "work-key", true).unwrap();
        assert!(list_provider_accounts(&memory, &provider)
            .unwrap()
            .selected()
            .is_none());
        assert!(remove_provider_account(
            &memory,
            &provider,
            LEGACY_API_KEY_ID,
            Some(LEGACY_API_KEY_ID)
        )
        .is_err());
        select_provider_account(&memory, &provider, &added.credential_id).unwrap();
        assert!(
            matches!(crate::resolve_provider_credential(&memory, "xai").unwrap(), crate::CredentialSource::AuthFile(stored) if stored.secret_account == added.credential_id)
        );
        assert!(remove_provider_account(&memory, &provider, &added.credential_id, None).is_err());
        remove_provider_account(&memory, &provider, LEGACY_API_KEY_ID, None).unwrap();
        remove_provider_account(&memory, &provider, &added.credential_id, None).unwrap();
        assert!(list_provider_accounts(&memory, &provider)
            .unwrap()
            .accounts
            .is_empty());
        let directory = std::env::temp_dir().join(format!(
            "pawork-accounts-{}-{}",
            std::process::id(),
            now_unix_millis()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("auth.json");
        // Independent backends use the same disk lock, just as separate hosts do.
        std::thread::scope(|scope| {
            for index in 0..4 {
                let path = &path;
                let provider = &provider;
                scope.spawn(move || {
                    add_api_key_account(
                        &FileBackend::with_path(path),
                        provider,
                        &format!("Key {index}"),
                        "fixture-key",
                        true,
                    )
                    .unwrap();
                });
            }
        });
        let file = FileBackend::with_path(&path);
        let before = list_provider_accounts(&file, &provider).unwrap();
        assert_eq!(before.accounts.len(), 4);
        assert!(before.selected().is_some());
        let oauth =
            add_oauth_account(&file, &provider, "OAuth A", &tokens("access-a"), true).unwrap();
        let second =
            add_oauth_account(&file, &provider, "OAuth B", &tokens("access-b"), true).unwrap();
        select_provider_account(&file, &provider, &second.credential_id).unwrap();
        let reopened = FileBackend::with_path(&path);
        assert_eq!(
            list_provider_accounts(&reopened, &provider)
                .unwrap()
                .selected()
                .unwrap()
                .credential_id,
            second.credential_id
        );
        assert_eq!(
            crate::resolve_oauth_credential(&oauth.stored, &file)
                .unwrap()
                .expose_secret(),
            "access-a"
        );
        let bytes = std::fs::read(&path).unwrap();
        assert!(file
            .transaction(&mut |transaction| {
                transaction.store("temporary", "secret", "value")?;
                Err(AuthError::NotFound)
            })
            .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        remove_all_provider_accounts(&file, &provider).unwrap();
        assert!(list_provider_accounts(&file, &provider)
            .unwrap()
            .accounts
            .is_empty());
        assert!(matches!(
            file.get(&oauth.stored.secret_service, &oauth.stored.secret_account),
            Err(AuthError::NotFound)
        ));
    }

    #[tokio::test]
    async fn refresh_cannot_resurrect_deleted_account_or_overwrite_relogin() {
        use std::sync::Arc;
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        for remove in [true, false] {
            let server = MockServer::start().await;
            let backend = Arc::new(MemoryBackend::new());
            let provider = ProviderId::new(if remove {
                "remove-race"
            } else {
                "relogin-race"
            });
            let account = add_oauth_account(
                backend.as_ref(),
                &provider,
                "Original",
                &tokens("old-access"),
                true,
            )
            .unwrap();
            let other = add_oauth_account(
                backend.as_ref(),
                &provider,
                "Other",
                &tokens("other-access"),
                false,
            )
            .unwrap();
            select_provider_account(backend.as_ref(), &provider, &other.credential_id).unwrap();
            let mutation_backend = backend.clone();
            let mutation_provider = provider.clone();
            let mutation_account = account.clone();
            Mock::given(method("POST")).respond_with(move |_: &wiremock::Request| {
                if remove {
                    remove_provider_account(mutation_backend.as_ref(), &mutation_provider, &mutation_account.credential_id, None).unwrap();
                } else {
                    // Same-slot relogin commits while the refresh request is in flight.
                    mutation_backend.transaction(&mut |transaction| {
                        store_oauth_at(transaction, mutation_provider.clone(), &mutation_account.credential_id, &tokens("relogin-access"))?;
                        Ok(())
                    }).unwrap();
                }
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"access_token":"late-access", "refresh_token":"late-refresh", "expires_in":3600, "token_type":"Bearer"}))
            }).expect(1).mount(&server).await;
            let mut stored = account.stored;
            let config = crate::OAuthRefreshConfig {
                token_url: server.uri(),
                client_id: "fixture".into(),
                refresh_skew: std::time::Duration::from_secs(30),
            };
            assert!(crate::refresh_default_oauth_credential_if_needed(
                &mut stored,
                backend.as_ref(),
                &config,
                &reqwest::Client::new()
            )
            .await
            .is_err());
            let current = backend.get(&stored.secret_service, &stored.secret_account);
            if remove {
                assert!(matches!(current, Err(AuthError::NotFound)));
            } else {
                assert_eq!(current.unwrap(), "relogin-access");
            }
            assert_eq!(
                crate::resolve_oauth_credential(&other.stored, backend.as_ref())
                    .unwrap()
                    .expose_secret(),
                "other-access"
            );
        }
    }
}
