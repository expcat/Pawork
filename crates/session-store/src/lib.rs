//! SQLite Session 存储。
//!
//! `session_events` 是事实来源；其他表均为可删除、可从事件重建的 Projection。

mod event_store;
mod migration;
mod projection;

use std::path::{Path, PathBuf};

use app_database::{DatabaseActor, DatabaseError};
pub use event_store::{AppendReceipt, DEFAULT_BRANCH_ID};
pub use migration::{MigrationReport, CURRENT_SCHEMA_VERSION};
pub use projection::{ProjectedRun, ProjectedToolCall, ProjectionSnapshot};
use thiserror::Error;

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
        Ok((Self { database, path }, report))
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
    #[error("database schema version {found} is newer than supported version {supported}")]
    UnsupportedSchema { found: u32, supported: u32 },
    #[error("schema version does not fit into u32: {0}")]
    InvalidSchemaVersion(i64),
    #[error("migration {version} ({name}) failed: {message}")]
    MigrationFailed {
        version: u32,
        name: &'static str,
        message: String,
    },
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("branch not found for session {session_id}: {branch_id}")]
    BranchNotFound {
        session_id: String,
        branch_id: String,
    },
    #[error("event belongs to session {event_session_id}, not {expected_session_id}")]
    EventSessionMismatch {
        expected_session_id: String,
        event_session_id: String,
    },
    #[error("event sequence is not contiguous: expected {expected}, got {actual}")]
    NonContiguousSequence { expected: u64, actual: u64 },
    #[error("event sequence overflow")]
    SequenceOverflow,
    #[error("parent event is missing from the same session: {0}")]
    ParentEventNotFound(String),
    #[error("projection invariant failed: {0}")]
    ProjectionInvariant(String),
}
