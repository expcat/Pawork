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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAccountSelectionMode {
    #[default]
    Manual,
    WhenExhausted,
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
    #[serde(default)]
    selection_mode: ProviderAccountSelectionMode,
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
    pub selection_mode: ProviderAccountSelectionMode,
    pub revision: u64,
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
        selection_mode: ProviderAccountSelectionMode::Manual,
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
            selection_mode: index.selection_mode,
            revision: index.revision,
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
        write_index(transaction, provider, &mut index)
    })?;
    result.ok_or_else(malformed)
}

fn write_index(
    transaction: &dyn SecretBackend,
    provider: &ProviderId,
    index: &mut AccountIndex,
) -> Result<(), AuthError> {
    index.revision = index.revision.checked_add(1).ok_or_else(malformed)?;
    let value = serde_json::to_string(index).map_err(|_| malformed())?;
    transaction.store(&secret_service_for(provider), INDEX_ACCOUNT, &value)
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
        index.selection_mode = ProviderAccountSelectionMode::Manual;
        Ok(())
    })
}

/// Persist selection mode after validating the selected stored API key in the transaction.
pub fn set_provider_account_selection_mode(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
    mode: ProviderAccountSelectionMode,
) -> Result<(), AuthError> {
    edit(backend, provider, |transaction, index| {
        if mode == ProviderAccountSelectionMode::WhenExhausted {
            if provider.as_str() != "opencode-go" {
                return Err(AuthError::InvalidSecret(
                    "automatic account selection requires opencode-go".into(),
                ));
            }
            let entry = index
                .accounts
                .iter()
                .find(|entry| {
                    Some(entry.credential_id.as_str()) == index.selected_credential_id.as_deref()
                        && entry.kind == ProviderAccountKind::ApiKey
                })
                .ok_or_else(|| {
                    AuthError::InvalidSecret(
                        "automatic account selection requires a selected stored API key".into(),
                    )
                })?;
            load_account(transaction, provider, entry)?;
        }
        index.selection_mode = mode;
        Ok(())
    })
}

/// Commit an automatic selection only while the observed account snapshot is current.
/// Conflicts leave both the selection and revision untouched.
pub fn select_provider_account_if_revision(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
    expected_revision: u64,
    expected_selected_id: &str,
    target_id: &str,
) -> Result<bool, AuthError> {
    let mut selected = false;
    backend.transaction(&mut |transaction| {
        let mut index = read_index(transaction, provider)?;
        if index.revision != expected_revision
            || index.selection_mode != ProviderAccountSelectionMode::WhenExhausted
            || index.selected_credential_id.as_deref() != Some(expected_selected_id)
        {
            return Ok(());
        }
        if provider.as_str() != "opencode-go" {
            return Err(malformed());
        }
        for id in [expected_selected_id, target_id] {
            let entry = index
                .accounts
                .iter()
                .find(|entry| entry.credential_id == id)
                .ok_or(AuthError::NotFound)?;
            if entry.kind != ProviderAccountKind::ApiKey {
                return Err(AuthError::InvalidSecret(
                    "automatic account selection requires stored API keys".into(),
                ));
            }
            load_account(transaction, provider, entry)?;
        }
        index.selected_credential_id = Some(target_id.into());
        write_index(transaction, provider, &mut index)?;
        selected = true;
        Ok(())
    })?;
    Ok(selected)
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
        if index.selected_credential_id.is_none() {
            index.selection_mode = ProviderAccountSelectionMode::Manual;
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
        index.selection_mode = ProviderAccountSelectionMode::Manual;
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
        if index.selected_credential_id.is_none() {
            index.selection_mode = ProviderAccountSelectionMode::Manual;
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

    #[test]
    fn automatic_selection_persists_and_rejects_stale_results() {
        use ProviderAccountSelectionMode::{Manual, WhenExhausted};
        let provider = ProviderId::new("opencode-go");
        let directory = std::env::temp_dir().join(format!(
            "pawork-selection-{}-{}",
            std::process::id(),
            now_unix_millis()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("auth.json");
        let backend = FileBackend::with_path(&path);
        let first = add_api_key_account(&backend, &provider, "First", "first-key", true).unwrap();
        let second =
            add_api_key_account(&backend, &provider, "Second", "second-key", false).unwrap();
        set_provider_account_selection_mode(&backend, &provider, WhenExhausted).unwrap();
        let reopened = FileBackend::with_path(&path);
        let a = first.credential_id.as_str();
        let b = second.credential_id.as_str();
        let auto_mode =
            || set_provider_account_selection_mode(&backend, &provider, WhenExhausted).unwrap();
        let cas = |revision, selected: &str, target: &str| {
            select_provider_account_if_revision(&reopened, &provider, revision, selected, target)
                .unwrap()
        };
        let before = list_provider_accounts(&reopened, &provider).unwrap();
        assert_eq!(before.selection_mode, WhenExhausted);
        assert_eq!(before.revision, 3);
        assert!(cas(before.revision, a, b));
        let after = list_provider_accounts(&backend, &provider).unwrap();
        assert_eq!(after.revision, before.revision + 1);
        assert_eq!(
            after.selected().unwrap().credential_id,
            second.credential_id
        );
        assert_eq!(after.selection_mode, WhenExhausted);
        let bytes = std::fs::read(&path).unwrap();
        assert!(!cas(before.revision, a, b));
        assert!(!cas(after.revision, a, b));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        // A manual selection of the same ID still invalidates an in-flight decision.
        select_provider_account(&backend, &provider, b).unwrap();
        let manual = list_provider_accounts(&reopened, &provider).unwrap();
        assert_eq!(manual.selection_mode, Manual);
        assert_eq!(manual.revision, after.revision + 1);
        assert!(!cas(after.revision, b, a));
        assert!(!cas(manual.revision, b, a));
        auto_mode();
        let before_delete = list_provider_accounts(&backend, &provider).unwrap();
        remove_provider_account(&reopened, &provider, a, None).unwrap();
        assert!(!cas(before_delete.revision, b, a));
        remove_provider_account(&backend, &provider, b, None).unwrap();
        let empty = list_provider_accounts(&reopened, &provider).unwrap();
        assert!(empty.selected().is_none());
        assert_eq!(empty.selection_mode, Manual);
        store_legacy_api_key(&backend, &provider, "legacy-key").unwrap();
        select_provider_account(&backend, &provider, LEGACY_API_KEY_ID).unwrap();
        auto_mode();
        remove_legacy(&backend, &provider, LEGACY_API_KEY_ID).unwrap();
        assert_eq!(
            list_provider_accounts(&backend, &provider)
                .unwrap()
                .selection_mode,
            Manual
        );
        add_api_key_account(&backend, &provider, "Again", "again-key", true).unwrap();
        auto_mode();
        remove_all_provider_accounts(&backend, &provider).unwrap();
        assert_eq!(
            list_provider_accounts(&reopened, &provider)
                .unwrap()
                .selection_mode,
            Manual
        );
    }

    #[test]
    fn automatic_selection_validation_does_not_mutate_index() {
        let backend = MemoryBackend::new();
        use ProviderAccountSelectionMode::{Manual, WhenExhausted};
        let provider = ProviderId::new("opencode-go");
        let service = secret_service_for(&provider);
        let first = add_api_key_account(&backend, &provider, "First", "first-key", false).unwrap();
        let initial = backend.get(&service, INDEX_ACCOUNT).unwrap();
        assert!(set_provider_account_selection_mode(&backend, &provider, WhenExhausted).is_err());
        assert_eq!(backend.get(&service, INDEX_ACCOUNT).unwrap(), initial);
        select_provider_account(&backend, &provider, &first.credential_id).unwrap();
        set_provider_account_selection_mode(&backend, &provider, WhenExhausted).unwrap();
        let oauth =
            add_oauth_account(&backend, &provider, "OAuth", &tokens("access"), false).unwrap();
        let second =
            add_api_key_account(&backend, &provider, "Second", "second-key", false).unwrap();
        backend.store(&service, &second.credential_id, "").unwrap();
        let revision = provider_accounts_revision(&backend, &provider)
            .unwrap()
            .unwrap();
        let initial = backend.get(&service, INDEX_ACCOUNT).unwrap();
        for target in [
            oauth.credential_id.as_str(),
            second.credential_id.as_str(),
            "cred_missing",
        ] {
            assert!(select_provider_account_if_revision(
                &backend,
                &provider,
                revision,
                &first.credential_id,
                target
            )
            .is_err());
            assert_eq!(backend.get(&service, INDEX_ACCOUNT).unwrap(), initial);
        }
        backend.store(&service, &first.credential_id, "").unwrap();
        assert!(set_provider_account_selection_mode(&backend, &provider, WhenExhausted).is_err());
        assert_eq!(backend.get(&service, INDEX_ACCOUNT).unwrap(), initial);
        let other = ProviderId::new("xai");
        add_api_key_account(&backend, &other, "Other", "other-key", true).unwrap();
        let initial = backend
            .get(&secret_service_for(&other), INDEX_ACCOUNT)
            .unwrap();
        assert!(set_provider_account_selection_mode(&backend, &other, WhenExhausted).is_err());
        assert_eq!(
            backend
                .get(&secret_service_for(&other), INDEX_ACCOUNT)
                .unwrap(),
            initial
        );
        set_provider_account_selection_mode(&backend, &other, Manual).unwrap();
        // Existing serialized indices omit the new field and must remain manual.
        let mut old: serde_json::Value = serde_json::from_str(&initial).unwrap();
        old.as_object_mut().unwrap().remove("selection_mode");
        backend
            .store(&secret_service_for(&other), INDEX_ACCOUNT, &old.to_string())
            .unwrap();
        assert_eq!(
            list_provider_accounts(&backend, &other)
                .unwrap()
                .selection_mode,
            Manual
        );
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
                ResponseTemplate::new(200).set_body_json(crate::testsupport::token_success_json("late-access", Some("late-refresh"), None))
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
