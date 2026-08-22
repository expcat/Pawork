//! SQLite Session 存储。
//!
//! `session_events` 是事实来源；其他表均为可删除、可从事件重建的 Projection。

mod catalog;
mod command_ledger;
mod client_adapter;
mod event_store;
pub mod import;
mod migration;
mod projection;
mod session_tree;

#[cfg(feature = "compaction")]
pub mod compaction;

#[cfg(test)]
pub(crate) mod test_support;

use std::path::{Path, PathBuf};

use crate::sqlite::{DatabaseActor, DatabaseError, MigrationError};
use thiserror::Error;

pub use catalog::SessionRecord;
pub use command_ledger::{
    CommandLedger, LedgerCheck, LedgerError, LedgerStats, WaitingToolCall,
    DEFAULT_COMMAND_LEDGER_CAPACITY,
};
pub use client_adapter::SqliteClientSessionRegistryStore;
pub use event_store::{AppendReceipt, DEFAULT_BRANCH_ID};
pub use session_tree::{BranchNode, SessionTree};
pub use import::{
    parse_pi_line, CompatImportHistoryEntry, CompatImportHistoryPage, CompatImportReport,
    ExportedBranch, ExportedEvent, ExternalRecord, ExternalSource, ParsedExternalSession,
    PiEntryKind, PiImportReport, PiParsedEntry, PiPayload, SessionExport, EXPORT_SCHEMA_VERSION,
};
pub use migration::CURRENT_SCHEMA_VERSION;
pub use crate::sqlite::MigrationReport;
pub use projection::{
    ProjectedProgramOutput, ProjectedRun, ProjectedScreenshot, ProjectedServerToolEvent,
    ProjectedToolCall, ProjectedTranscriptEnvelope, ProjectionSnapshot,
};
#[cfg(feature = "compaction")]
pub use compaction::*;

#[derive(Clone)]
pub struct SessionStore {
    database: DatabaseActor,
    path: PathBuf,
}

impl SessionStore {
    pub async fn open(
        path: impl Into<PathBuf>,
    ) -> Result<(Self, MigrationReport), SessionStoreError> {
        let path = path.into();
        let existed = path.exists()
            && path
                .metadata()
                .map(|metadata| metadata.len() > 0)
                .unwrap_or(false);
        let database = DatabaseActor::open(&path).await?;
        let report = migration::migrate(&database, &path, existed).await?;
        let store = Self { database, path };
        // 单宿主进程模型：写打开后回收上次崩溃遗留的 inflight 占位。
        store.command_ledger().reclaim_inflight().await?;
        Ok((store, report))
    }

    pub async fn open_read_only(path: impl Into<PathBuf>) -> Result<Self, SessionStoreError> {
        let path = path.into();
        let database = DatabaseActor::open_read_only(&path).await?;
        let version = migration::schema_version(&database).await?;
        if version != CURRENT_SCHEMA_VERSION {
            return Err(SessionStoreError::UnsupportedSchema {
                found: version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }
        Ok(Self { database, path })
    }

    pub fn database(&self) -> &DatabaseActor {
        &self.database
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn schema_version(&self) -> Result<u32, SessionStoreError> {
        migration::schema_version(&self.database).await
    }

    pub async fn shutdown(self) -> Result<(), SessionStoreError> {
        self.database.shutdown().await?;
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum SessionStoreError {
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Ledger(#[from] command_ledger::LedgerError),
    #[error("database schema version {found} is newer than supported version {supported}")]
    UnsupportedSchema { found: u32, supported: u32 },
    #[error("export schema version {found} is not supported (expected {supported})")]
    ExportSchemaVersion { found: u32, supported: u32 },
    #[error("session export v3 identity is missing or blank")]
    ExportIdentityMissing,
    #[error(
        "session export identity {export_tenant}/{export_principal} does not match import identity {import_tenant}/{import_principal}"
    )]
    ExportIdentityMismatch {
        export_tenant: String,
        export_principal: String,
        import_tenant: String,
        import_principal: String,
    },
    #[error("event belongs to session {event_session_id}, not {expected_session_id}")]
    EventSessionMismatch {
        expected_session_id: String,
        event_session_id: String,
    },
    #[error("schema version does not fit into u32: {0}")]
    InvalidSchemaVersion(i64),
    #[error("migration {version} ({name}) failed: {message}")]
    MigrationFailed {
        version: u32,
        name: String,
        message: String,
    },
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("branch not found for session {session_id}: {branch_id}")]
    BranchNotFound {
        session_id: String,
        branch_id: String,
    },
    #[error("branch already exists for session {session_id}: {branch_id}")]
    BranchAlreadyExists {
        session_id: String,
        branch_id: String,
    },
    #[error(
        "branch {requested_branch} is not the active branch of session {session_id}; active is {active_branch}"
    )]
    BranchNotActive {
        session_id: String,
        active_branch: String,
        requested_branch: String,
    },
    #[error("session {session_id} still has persisted events; archive instead of deleting")]
    SessionHasEvents { session_id: String },
    #[error("lease for session {session_id} is held by {holder} until {expires_at_ms}ms")]
    LeaseHeld {
        session_id: String,
        holder: String,
        expires_at_ms: i64,
    },
    #[error("no lease is held for session {session_id} by the requested holder")]
    LeaseNotHeld { session_id: String },
    #[error("event sequence is not contiguous: expected {expected}, got {actual}")]
    NonContiguousSequence { expected: u64, actual: u64 },
    #[error("event sequence overflow")]
    SequenceOverflow,
    #[error("parent event is missing from the same session: {0}")]
    ParentEventNotFound(String),
    #[error("projection invariant failed: {0}")]
    ProjectionInvariant(String),
    #[error("compat import source could not be parsed ({source_label}): {detail}")]
    CompatUnparseable {
        source_label: String,
        detail: String,
    },
    #[error("compat import source contains a likely secret ({pattern}); nothing imported")]
    CompatSecretDetected { pattern: String },
    #[error("compat import replay validation failed: {0}")]
    CompatValidationFailed(String),
    #[error(
        "compat import identity conflict for source {source_label} / original_id {original_id}: \
         same identity already imported with different content; refusing to create a second session"
    )]
    CompatImportConflict {
        source_label: String,
        original_id: String,
    },
    #[error("compat import history cursor is malformed: {0}")]
    InvalidHistoryCursor(String),
    #[error("compat import history contains unknown source label `{0}`")]
    InvalidHistorySource(String),
}

impl From<MigrationError> for SessionStoreError {
    fn from(error: MigrationError) -> Self {
        match error {
            MigrationError::Database(error) => Self::Database(error),
            MigrationError::Sqlite(error) => Self::Sqlite(error),
            MigrationError::UnsupportedSchema { found, supported } => Self::UnsupportedSchema {
                found: u32::try_from(found).unwrap_or(u32::MAX),
                supported,
            },
            MigrationError::MigrationFailed {
                version,
                name,
                message,
            } => Self::MigrationFailed {
                version,
                name,
                message,
            },
            MigrationError::InvalidSchemaVersion(version) => Self::InvalidSchemaVersion(version),
            other => Self::MigrationFailed {
                version: 0,
                name: "migration".into(),
                message: other.to_string(),
            },
        }
    }
}
