//! `pawork-storage::blob`：内容寻址 Artifact、Protected Blob（PWB1）与 Checkpoint 快照。
//!
//! 默认面是 [`ArtifactStore`]（`artifacts.sqlite3` + `blobs/`）。
//! `protected` feature 提供 PWB1 AEAD 层（`protected.sqlite3` + `protected/`）；
//! `checkpoint` feature 提供写前快照 / 回滚（`checkpoint-state-v1.json`）。

pub mod artifact;

pub use artifact::{
    ArtifactStore, ArtifactStoreError, ArtifactStoreOptions, BlobId, BlobMetadata, GcReport,
    IntegrityReport, PutOutcome,
};

#[cfg(feature = "protected")]
pub mod protected;
#[cfg(feature = "protected")]
pub use pawork_domain::{ProtectedBlobRef, ProviderId, SessionId};
#[cfg(feature = "protected")]
pub use protected::{
    open_pwb1_envelope, parse_pwb1_envelope, pwb1_aad, AeadKey, BlobScope, InMemoryKeyResolver,
    KeyResolutionError, KeyVersion, ProtectedBlob, ProtectedBlobError, ProtectedBlobMetadata,
    ProtectedBlobStore, ProtectedBlobStoreOptions, ProtectedKeyResolver, PWB1_ALGORITHM,
    PWB1_HEADER_LEN, PWB1_MAGIC, PWB1_NONCE_LEN, PWB1_VERSION,
};

#[cfg(feature = "checkpoint")]
pub mod checkpoint;
#[cfg(feature = "checkpoint")]
pub use checkpoint::{
    ChangeRecord, CheckpointError, CheckpointService, ConflictReport, FileSnapshot, RunCheckpoint,
};
